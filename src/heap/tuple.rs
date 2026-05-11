use crate::{
    file::error::{self, PgError, Result},
    types::{PgAlign, PgDatum, PgSchema, decode_datum, skip_datum},
};
use derive_where::derive_where;
use log::error;

/// Block identifier — two halves of a 32-bit block number. Corresponds to `BlockIdData`.
#[derive(Debug, Clone, Copy)]
pub struct BlockIdData {
    pub bi_hi: u16,
    pub bi_lo: u16,
}

impl BlockIdData {
    pub fn block_number(&self) -> u32 {
        u32::from(*self)
    }
}

impl From<u32> for BlockIdData {
    fn from(n: u32) -> Self {
        BlockIdData {
            bi_hi: (n >> 16) as u16,
            bi_lo: n as u16,
        }
    }
}

impl From<BlockIdData> for u32 {
    fn from(b: BlockIdData) -> Self {
        ((b.bi_hi as u32) << 16) | (b.bi_lo as u32)
    }
}

/// Tuple identifier (TID) — block + offset within page. Corresponds to `ItemPointerData`. 6 bytes.
#[derive(Debug, Clone, Copy)]
pub struct ItemPointerData {
    pub ip_blkid: BlockIdData,
    pub ip_posid: u16,
}

/// Heap tuple header (23 bytes fixed). Corresponds to `HeapTupleHeaderData`.
/// Null bitmap (`t_bits`) follows after `t_hoff`; actual data follows after the bitmap.
#[derive(Debug, Clone, Copy)]
pub struct HeapTupleHeaderData {
    // t_choice union — HeapTupleFields interpretation:
    pub t_xmin: u32,
    pub t_xmax: u32,
    pub t_field3: u32, // union: t_cid (CommandId) or t_xvac (TransactionId)
    pub t_ctid: ItemPointerData,
    pub t_infomask2: u16,
    pub t_infomask: u16,
    pub t_hoff: u8,
    // t_bits[FLEXIBLE_ARRAY_MEMBER] follows
}

impl HeapTupleHeaderData {
    /// Test whether a flag or bitmask is set in `t_infomask`.
    /// Accepts both [`InfoMask`] variants and `u16` composite constants (e.g. `InfoMask::XMIN_FROZEN`).
    pub fn has_flag(&self, flag: impl Into<u16>) -> bool {
        self.t_infomask & flag.into() != 0
    }
}

impl std::fmt::Display for HeapTupleHeaderData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let natts = self.t_infomask2 & HEAP_NATTS_MASK;
        let ctid_block = self.t_ctid.ip_blkid.block_number();

        writeln!(f, "HeapTupleHeader {{")?;
        writeln!(f, "  xmin:      {}", self.t_xmin)?;
        writeln!(f, "  xmax:      {}", self.t_xmax)?;
        writeln!(f, "  field3:    {}  (cid/xvac)", self.t_field3)?;
        writeln!(f, "  ctid:      ({}, {})", ctid_block, self.t_ctid.ip_posid)?;
        writeln!(f, "  natts:     {}", natts)?;
        writeln!(f, "  t_hoff:    {}", self.t_hoff)?;

        // Decode t_infomask2 flag bits (natts occupies lower 11 bits, reported above).
        let mut mask2_flags: Vec<&str> = Vec::new();
        if self.t_infomask2 & HEAP_KEYS_UPDATED != 0 {
            mask2_flags.push("KEYS_UPDATED");
        }
        if self.t_infomask2 & HEAP_HOT_UPDATED != 0 {
            mask2_flags.push("HOT_UPDATED");
        }
        if self.t_infomask2 & HEAP_ONLY_TUPLE != 0 {
            mask2_flags.push("ONLY_TUPLE");
        }
        if mask2_flags.is_empty() {
            writeln!(
                f,
                "  infomask2: 0x{:04X}  (natts={})",
                self.t_infomask2, natts
            )?;
        } else {
            writeln!(
                f,
                "  infomask2: 0x{:04X}  [ {} ]  (natts={})",
                self.t_infomask2,
                mask2_flags.join(" | "),
                natts
            )?;
        }

        // Decode t_infomask flag bits.
        const ALL_FLAGS: &[InfoMask] = &[
            InfoMask::HasNull,
            InfoMask::HasVarWidth,
            InfoMask::HasExternal,
            InfoMask::HasOidOld,
            InfoMask::XmaxKeyshrLock,
            InfoMask::ComboCid,
            InfoMask::XmaxExclLock,
            InfoMask::XmaxLockOnly,
            InfoMask::XminCommitted,
            InfoMask::XminInvalid,
            InfoMask::XmaxCommitted,
            InfoMask::XmaxInvalid,
            InfoMask::XmaxIsMulti,
            InfoMask::Updated,
            InfoMask::MovedOff,
            InfoMask::MovedIn,
        ];
        let set_flags: Vec<&str> = ALL_FLAGS
            .iter()
            .filter(|&&flag| self.has_flag(flag))
            .map(|flag| flag.short_name())
            .collect();
        if set_flags.is_empty() {
            writeln!(f, "  infomask:  0x{:04X}  (none)", self.t_infomask)?;
        } else {
            writeln!(
                f,
                "  infomask:  0x{:04X}  [ {} ]",
                self.t_infomask,
                set_flags.join(" | ")
            )?;
        }

        // Synthetic annotations derived from flag combinations.
        if self.has_flag(InfoMask::XMIN_FROZEN) {
            writeln!(f, "  [xmin is frozen]")?;
        }

        write!(f, "}}")
    }
}

#[derive(Clone)]
#[derive_where(Debug)]
pub struct HeapTupleData {
    /// Postgres tuple header.
    pub header: HeapTupleHeaderData,
    /// Raw bytes of the null bitmap (`t_bits`), i.e. `raw[23..t_hoff]`.
    /// Non-empty only when `header.has_flag(InfoMask::HasNull)` is true.
    /// Each bit i (LSB-first within each byte) represents attribute i:
    /// `1` = NOT NULL, `0` = NULL.
    pub null_bitmap: Vec<u8>,

    /// Attribute data bytes, starting at `t_hoff` (past the null bitmap).
    #[derive_where(skip)]
    pub data: Vec<u8>,
}

impl HeapTupleData {
    /// Parse raw tuple bytes from a heap page into a [`HeapTupleData`].
    ///
    /// `raw` is the slice starting at the tuple's offset within the page
    /// (i.e. `page_data[lp_off .. lp_off + lp_len]`).
    ///
    /// Returns an error if the buffer is too short, or if `t_hoff` falls
    /// outside the valid range `[SIZEOF_HEAP_TUPLE_HEADER, raw.len()]`.
    pub fn parse_and_build(raw: &[u8]) -> error::Result<Self> {
        if raw.len() < SIZEOF_HEAP_TUPLE_HEADER {
            error!(
                "tuple buffer too short: got {} bytes, need at least {}",
                raw.len(),
                SIZEOF_HEAP_TUPLE_HEADER
            );
            return Err(error::PgError::TupleBufferTooShort {
                actual: raw.len(),
                expected: SIZEOF_HEAP_TUPLE_HEADER,
            });
        }

        // SAFETY: all slices are within [0, SIZEOF_HEAP_TUPLE_HEADER) which we
        // validated above. Fixed-size array conversions cannot fail for the
        // exact lengths used here.
        let t_xmin = u32::from_ne_bytes(raw[0..4].try_into().unwrap());
        let t_xmax = u32::from_ne_bytes(raw[4..8].try_into().unwrap());
        let t_field3 = u32::from_ne_bytes(raw[8..12].try_into().unwrap());

        // t_ctid: BlockIdData (4 bytes) + ip_posid (2 bytes)
        let bi_hi = u16::from_ne_bytes(raw[12..14].try_into().unwrap());
        let bi_lo = u16::from_ne_bytes(raw[14..16].try_into().unwrap());
        let ip_posid = u16::from_ne_bytes(raw[16..18].try_into().unwrap());

        let t_infomask2 = u16::from_ne_bytes(raw[18..20].try_into().unwrap());
        let t_infomask = u16::from_ne_bytes(raw[20..22].try_into().unwrap());
        let t_hoff = raw[22];

        let t_hoff_usize = t_hoff as usize;
        if t_hoff_usize < SIZEOF_HEAP_TUPLE_HEADER || t_hoff_usize > raw.len() {
            error!(
                "invalid t_hoff {}: must be in [{}, {}]",
                t_hoff,
                SIZEOF_HEAP_TUPLE_HEADER,
                raw.len()
            );
            return Err(error::PgError::InvalidTupleHoff {
                t_hoff,
                min: SIZEOF_HEAP_TUPLE_HEADER,
                buf_len: raw.len(),
            });
        }

        let header = HeapTupleHeaderData {
            t_xmin,
            t_xmax,
            t_field3,
            t_ctid: ItemPointerData {
                ip_blkid: BlockIdData { bi_hi, bi_lo },
                ip_posid,
            },
            t_infomask2,
            t_infomask,
            t_hoff,
        };

        // Null bitmap: exactly ⌈natts/8⌉ bytes starting at byte 23.
        // raw[23..t_hoff] also contains MAXALIGN padding after the bitmap,
        // so use natts to copy only the meaningful bytes.
        let null_bitmap = if header.has_flag(InfoMask::HasNull) {
            let natts = (header.t_infomask2 & HEAP_NATTS_MASK) as usize;
            let bitmap_bytes = natts.div_ceil(8);
            let bitmap_end = SIZEOF_HEAP_TUPLE_HEADER + bitmap_bytes;
            if bitmap_end > t_hoff_usize {
                error!(
                    "null bitmap overflows t_hoff: bitmap_end={} > t_hoff={}",
                    bitmap_end, t_hoff_usize
                );
                return Err(error::PgError::NullBitmapOverflow {
                    bitmap_end,
                    t_hoff: t_hoff_usize,
                });
            }
            raw[SIZEOF_HEAP_TUPLE_HEADER..bitmap_end].to_vec()
        } else {
            Vec::new()
        };

        // Attribute data begins at t_hoff (past the fixed header and null bitmap).
        let data = raw[t_hoff_usize..].to_vec();

        Ok(HeapTupleData {
            header,
            null_bitmap,
            data,
        })
    }

    pub fn get_column(
        &self,
        target_column: ColumnSearchArg,
        schema: &PgSchema,
        column_offset: usize,
    ) -> Result<(PgDatum, usize)> {
        // Resolve the target column to an index and its metadata.
        // log::debug!("{:?} {:?} {}", target_column, schema, column_offset);
        let (column_index, col) = match &target_column {
            ColumnSearchArg::ColumnIndex(idx) => {
                let col = schema.column(*idx).unwrap();
                // .ok_or(PgError::ColumnNotFound {
                //           column: format!("index {idx}"),
                //       })?;
                (*idx, col)
            }
            ColumnSearchArg::ColumnName(name) => {
                let (idx, col) = schema
                    .columns()
                    .enumerate()
                    .find(|(_, c)| c.name == *name)
                    .ok_or(PgError::ColumnNotFound {
                        column: name.clone(),
                    })?;
                (idx, col)
            }
        };

        // log::debug!(
        //     "get_column: {:?} → index={}, type={:?}, null_byte={:#08b}",
        //     target_column,
        //     column_index,
        //     col.type_id,
        //     self.null_bitmap
        //         .get(column_index / 8)
        //         .copied()
        //         .unwrap_or(0xFF)
        // );
        //
        // Check null bitmap: bit=0 means NULL in PostgreSQL convention.
        if self
            .null_bitmap
            .get(column_index / 8)
            .map(|b| b & (1u8 << (column_index % 8)) == 0)
            .unwrap_or(false)
        {
            return Err(PgError::NullColumnValue { id: column_index });
        }

        let res = decode_datum(col.type_id, &self.data, column_offset)
            .map_err(|e| PgError::DecodeError(e.to_string()))?;

        Ok(res)
    }

    /// Check if a column is NULL according to the null bitmap.
    /// Returns `true` if the column is NULL (bit=0) or the bitmap is empty
    /// and `HasNull` is not set (never null).
    pub fn is_null(&self, col_index: usize) -> bool {
        self.null_bitmap
            .get(col_index / 8)
            .map(|b| b & (1u8 << (col_index % 8)) == 0)
            .unwrap_or(false)
    }

    /// Compute the byte size of a column without decoding its value.
    ///
    /// Returns `Ok(size)` for non-NULL columns, or `Err(NullColumnValue)` for NULL.
    /// Used by projected decoding to advance the offset past skipped columns.
    pub fn skip_column(
        &self,
        col_index: usize,
        schema: &PgSchema,
        column_offset: usize,
    ) -> Result<usize> {
        let col = schema.column(col_index).ok_or(PgError::ColumnNotFound {
            column: format!("index {col_index}"),
        })?;

        // Check null bitmap: bit=0 means NULL in PostgreSQL convention.
        if self
            .null_bitmap
            .get(col_index / 8)
            .map(|b| b & (1u8 << (col_index % 8)) == 0)
            .unwrap_or(false)
        {
            return Err(PgError::NullColumnValue { id: col_index });
        }

        skip_datum(col.type_id.type_len(), &self.data, column_offset)
            .map_err(|e| PgError::DecodeError(e.to_string()))
    }
}

#[derive(Debug, Clone)]
pub enum ColumnSearchArg {
    ColumnIndex(usize),
    ColumnName(String),
}

impl std::fmt::Display for HeapTupleData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.header)?;

        let natts = (self.header.t_infomask2 & HEAP_NATTS_MASK) as usize;

        if self.header.has_flag(InfoMask::HasNull) {
            writeln!(f, "\n  t_bits ({} attrs):", natts)?;
            for i in 0..natts {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                // PostgreSQL stores 1 = NOT NULL, 0 = NULL (att_isnull checks for 0).
                let not_null = self
                    .null_bitmap
                    .get(byte_idx)
                    .map(|b| b & (1u8 << bit_idx) != 0)
                    .unwrap_or(true);
                writeln!(
                    f,
                    "    attr[{:3}]: {}",
                    i,
                    if not_null { "NOT NULL" } else { "NULL    " }
                )?;
            }
        } else {
            write!(
                f,
                "\n  t_bits: (no null bitmap — all {} attrs NOT NULL)",
                natts
            )?;
        }

        Ok(())
    }
}

pub const SIZEOF_HEAP_TUPLE_HEADER: usize = 23;

/// Bit flags for `HeapTupleHeaderData::t_infomask`. Corresponds to PostgreSQL `HEAP_*` macros.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoMask {
    HasNull = 0x0001,
    HasVarWidth = 0x0002,
    HasExternal = 0x0004,
    HasOidOld = 0x0008,
    XmaxKeyshrLock = 0x0010,
    ComboCid = 0x0020,
    XmaxExclLock = 0x0040,
    XmaxLockOnly = 0x0080,
    XminCommitted = 0x0100,
    XminInvalid = 0x0200,
    XmaxCommitted = 0x0400,
    XmaxInvalid = 0x0800,
    XmaxIsMulti = 0x1000,
    Updated = 0x2000,
    MovedOff = 0x4000,
    MovedIn = 0x8000,
}

impl InfoMask {
    /// xmax is a shared locker (EXCL | KEYSHR).
    pub const XMAX_SHR_LOCK: u16 = InfoMask::XmaxExclLock as u16 | InfoMask::XmaxKeyshrLock as u16;
    /// All lock-type bits.
    pub const LOCK_MASK: u16 =
        Self::XMAX_SHR_LOCK | InfoMask::XmaxExclLock as u16 | InfoMask::XmaxKeyshrLock as u16;
    /// Both COMMITTED and INVALID set → frozen xmin.
    pub const XMIN_FROZEN: u16 = InfoMask::XminCommitted as u16 | InfoMask::XminInvalid as u16;
    /// Either MOVED_OFF or MOVED_IN (pre-9.0 legacy).
    pub const MOVED: u16 = InfoMask::MovedOff as u16 | InfoMask::MovedIn as u16;
    /// All transaction-visibility bits.
    pub const XACT_MASK: u16 = 0xFFF0;

    /// Short flag name used in Display output (e.g. `"HASNULL"`).
    pub fn short_name(self) -> &'static str {
        match self {
            InfoMask::HasNull => "HASNULL",
            InfoMask::HasVarWidth => "HASVARWIDTH",
            InfoMask::HasExternal => "HASEXTERNAL",
            InfoMask::HasOidOld => "HASOID_OLD",
            InfoMask::XmaxKeyshrLock => "XMAX_KEYSHR_LOCK",
            InfoMask::ComboCid => "COMBOCID",
            InfoMask::XmaxExclLock => "XMAX_EXCL_LOCK",
            InfoMask::XmaxLockOnly => "XMAX_LOCK_ONLY",
            InfoMask::XminCommitted => "XMIN_COMMITTED",
            InfoMask::XminInvalid => "XMIN_INVALID",
            InfoMask::XmaxCommitted => "XMAX_COMMITTED",
            InfoMask::XmaxInvalid => "XMAX_INVALID",
            InfoMask::XmaxIsMulti => "XMAX_IS_MULTI",
            InfoMask::Updated => "UPDATED",
            InfoMask::MovedOff => "MOVED_OFF",
            InfoMask::MovedIn => "MOVED_IN",
        }
    }
}

impl From<InfoMask> for u16 {
    fn from(m: InfoMask) -> u16 {
        m as u16
    }
}

impl std::fmt::Display for InfoMask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            InfoMask::HasNull => "HASNULL",
            InfoMask::HasVarWidth => "HASVARWIDTH",
            InfoMask::HasExternal => "HASEXTERNAL",
            InfoMask::HasOidOld => "HASOID_OLD",
            InfoMask::XmaxKeyshrLock => "XMAX_KEYSHR_LOCK",
            InfoMask::ComboCid => "COMBOCID",
            InfoMask::XmaxExclLock => "XMAX_EXCL_LOCK",
            InfoMask::XmaxLockOnly => "XMAX_LOCK_ONLY",
            InfoMask::XminCommitted => "XMIN_COMMITTED",
            InfoMask::XminInvalid => "XMIN_INVALID",
            InfoMask::XmaxCommitted => "XMAX_COMMITTED",
            InfoMask::XmaxInvalid => "XMAX_INVALID",
            InfoMask::XmaxIsMulti => "XMAX_IS_MULTI",
            InfoMask::Updated => "UPDATED",
            InfoMask::MovedOff => "MOVED_OFF",
            InfoMask::MovedIn => "MOVED_IN",
        };
        write!(f, "InfoMask::{}", name)
    }
}

// t_infomask2 masks / flags
pub const HEAP_NATTS_MASK: u16 = 0x07FF;
/// Tuple was updated and key columns were modified, or tuple was deleted.
pub const HEAP_KEYS_UPDATED: u16 = 0x2000;
/// Tuple was HOT-updated (successor is on the same page).
pub const HEAP_HOT_UPDATED: u16 = 0x4000;
/// This tuple is a heap-only tuple (HOT chain member, no index entry).
pub const HEAP_ONLY_TUPLE: u16 = 0x8000;

/// Minimal column schema info, mirrors relevant fields from pg_attribute.
#[derive(Debug, Clone)]
pub struct PgAttInfo {
    pub attlen: i16, // -1 = varlena, -2 = cstring, >0 = fixed width
    pub attalign: PgAlign,
}

pub fn align_to(offset: usize, align: usize) -> usize {
    (offset + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use crate::types::PgDatum;
    use crate::table::PgTableReader;
    use crate::util::pg_harness;


    /// Decode validation: scan the decode test table via raw HeapTupleData iteration,
    /// decode each column via `get_column`, and compare against live PostgreSQL results.
    ///
    /// This validates that our low-level tuple decoder produces the same values as
    /// the authoritative source (PostgreSQL itself) for all basic types.
    #[test]
    fn test_decode_columns_match_live_postgres() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // ── 1. Connect to live PG and ensure the decode test table exists ──
            let client = pg_harness::connect().await;
            pg_harness::ensure_decode_test_table(&client).await;

            // ── 2. Read expected values from live PostgreSQL ───────────────────
            let pg_rows = client
                .query(
                    &format!(
                        "SELECT id, col_bool, col_int2, col_int4, col_int8,
                                col_float4, col_float8, col_text, col_bytea,
                                col_date::text, col_ts::text,
                                (col_tstz AT TIME ZONE 'UTC')::text as col_tstz_utc
                         FROM {} ORDER BY id",
                        pg_harness::DECODE_TEST_TABLE
                    ),
                    &[],
                )
                .await
                .expect("SELECT failed");

            assert!(!pg_rows.is_empty(), "decode test table should have rows");

            // ── 3. Read the same table via PgTableReader ──────────────────────
            let db_id = pg_harness::db_oid(&client, "postgres").await;
            let mut reader = PgTableReader::new(db_id).unwrap();
            reader
                .set_table(pg_harness::DECODE_TEST_TABLE)
                .expect("table not found — was setup run?");

            let decoded_rows = reader
                .fetch_all()
                .expect("failed to read decode test table");

            let schema = reader.schema().unwrap().clone();

            // Sort decoded rows by id (col index 0 = serial id).
            let mut decoded_rows = decoded_rows;
            decoded_rows.sort_by_key(|row| match row.get(0) {
                Some(PgDatum::Int4(id)) => *id,
                _ => i32::MAX,
            });

            assert_eq!(
                decoded_rows.len(),
                pg_rows.len(),
                "row count mismatch: pgfusion={} pg={}",
                decoded_rows.len(),
                pg_rows.len()
            );

            // Column name → schema index helper.
            let col_idx = |name: &str| -> usize {
                schema
                    .columns()
                    .enumerate()
                    .find(|(_, c)| c.name == name)
                    .map(|(i, _)| i)
                    .unwrap_or_else(|| panic!("column {name} not found in schema"))
            };

            let idx_bool = col_idx("col_bool");
            let idx_int2 = col_idx("col_int2");
            let idx_int4 = col_idx("col_int4");
            let idx_int8 = col_idx("col_int8");
            let idx_float4 = col_idx("col_float4");
            let idx_float8 = col_idx("col_float8");
            let idx_text = col_idx("col_text");
            let idx_bytea = col_idx("col_bytea");
            let idx_date = col_idx("col_date");
            let idx_ts = col_idx("col_ts");
            let idx_tstz = col_idx("col_tstz");

            for (i, (decoded, pg)) in decoded_rows.iter().zip(pg_rows.iter()).enumerate() {
                let label = format!("row {i}");

                // col_bool
                let want_bool = pg_harness::pg_bool(pg, "col_bool");
                let got_bool = match decoded.get(idx_bool) {
                    Some(PgDatum::Bool(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_bool unexpected datum {other:?}"),
                };
                assert_eq!(got_bool, want_bool, "{label}: col_bool");

                // col_int2
                let want_int2 = pg_harness::pg_i16(pg, "col_int2");
                let got_int2 = match decoded.get(idx_int2) {
                    Some(PgDatum::Int2(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_int2 unexpected datum {other:?}"),
                };
                assert_eq!(got_int2, want_int2, "{label}: col_int2");

                // col_int4
                let want_int4 = pg_harness::pg_i32(pg, "col_int4");
                let got_int4 = match decoded.get(idx_int4) {
                    Some(PgDatum::Int4(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_int4 unexpected datum {other:?}"),
                };
                assert_eq!(got_int4, want_int4, "{label}: col_int4");

                // col_int8
                let want_int8 = pg_harness::pg_i64(pg, "col_int8");
                let got_int8 = match decoded.get(idx_int8) {
                    Some(PgDatum::Int8(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_int8 unexpected datum {other:?}"),
                };
                assert_eq!(got_int8, want_int8, "{label}: col_int8");

                // col_float4
                let want_f4 = pg_harness::pg_f32(pg, "col_float4");
                let got_f4 = match decoded.get(idx_float4) {
                    Some(PgDatum::Float4(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_float4 unexpected datum {other:?}"),
                };
                assert_eq!(got_f4, want_f4, "{label}: col_float4");

                // col_float8
                let want_f8 = pg_harness::pg_f64(pg, "col_float8");
                let got_f8 = match decoded.get(idx_float8) {
                    Some(PgDatum::Float8(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_float8 unexpected datum {other:?}"),
                };
                assert_eq!(got_f8, want_f8, "{label}: col_float8");

                // col_text
                let want_text = pg_harness::pg_str(pg, "col_text");
                let got_text = match decoded.get(idx_text) {
                    Some(PgDatum::Text(v)) => Some(v.clone()),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_text unexpected datum {other:?}"),
                };
                assert_eq!(got_text, want_text, "{label}: col_text");

                // col_bytea
                let want_bytea = pg_harness::pg_bytes(pg, "col_bytea");
                let got_bytea = match decoded.get(idx_bytea) {
                    Some(PgDatum::Bytea(v)) => Some(v.clone()),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_bytea unexpected datum {other:?}"),
                };
                assert_eq!(got_bytea, want_bytea, "{label}: col_bytea");

                // col_date — compare as days since 1970-01-01
                let want_date = pg_harness::pg_date_days(pg, "col_date");
                let got_date = match decoded.get(idx_date) {
                    Some(PgDatum::Date(v)) => Some(*v + 10957),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_date unexpected datum {other:?}"),
                };
                assert_eq!(got_date, want_date, "{label}: col_date");

                // col_ts — compare as µs since 1970-01-01
                let want_ts = pg_harness::pg_ts_us(pg, "col_ts");
                let got_ts = match decoded.get(idx_ts) {
                    Some(PgDatum::Timestamp(v)) => Some(*v + 946_684_800_000_000),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_ts unexpected datum {other:?}"),
                };
                assert_eq!(got_ts, want_ts, "{label}: col_ts");

                // col_tstz
                let want_tstz = pg_harness::pg_ts_us(pg, "col_tstz_utc");
                let got_tstz = match decoded.get(idx_tstz) {
                    Some(PgDatum::TimestampTz(v)) => Some(*v + 946_684_800_000_000),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("{label}: col_tstz unexpected datum {other:?}"),
                };
                assert_eq!(got_tstz, want_tstz, "{label}: col_tstz");
            }
        });
    }
}
