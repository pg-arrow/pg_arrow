use std::sync::Arc;

use arrow::record_batch::RecordBatch;
use bytes::Bytes;
use derive_where::derive_where;

use crate::codec::{PgTypeId, PgTypeLen, skip_datum};
use crate::file::error;
use crate::heap::tuple::{HEAP_NATTS_MASK, HeapTupleData, InfoMask, SIZEOF_HEAP_TUPLE_HEADER};
use crate::table::{ColumnBuilder, extract_column_bytes, extract_fixed_bytes};
use crate::types::PgSchema;

pub const PAGE_BUFFER_SIZE: usize = 8 * 1024;
pub const PAGES_PER_SEGMENT: usize = (1024 * 1024 * 1024) / PAGE_BUFFER_SIZE;

/// PostgreSQL LSN stored as two 32-bit values. Corresponds to `PageXLogRecPtr`.
#[derive(Debug, Clone, Copy)]
pub struct PageXLogRecPtr {
    pub xlogid: u32,
    pub xrecoff: u32,
}

/// Line pointer (4 bytes). Corresponds to `ItemIdData`.
/// Bitfield: lp_off (15 bits) | lp_flags (2 bits) | lp_len (15 bits).
#[derive(Debug, Clone, Copy)]
pub struct ItemIdData(pub u32);

impl ItemIdData {
    /// Offset to tuple from start of page.
    pub fn lp_off(&self) -> u16 {
        (self.0 & 0x7FFF) as u16
    }

    /// State of line pointer.
    pub fn lp_flags(&self) -> u8 {
        ((self.0 >> 15) & 0x03) as u8
    }

    /// Byte length of tuple.
    pub fn lp_len(&self) -> u16 {
        ((self.0 >> 17) & 0x7FFF) as u16
    }
}

impl std::fmt::Display for ItemIdData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let flags = match self.lp_flags() {
            0 => "UNUSED",
            1 => "NORMAL",
            2 => "REDIRECT",
            3 => "DEAD",
            _ => "UNKNOWN",
        };
        write!(
            f,
            "ItemId(off={}, flags={}, len={})",
            self.lp_off(),
            flags,
            self.lp_len()
        )
    }
}

/// Page header fixed portion (24 bytes). Corresponds to `PageHeaderData`.
/// Line pointer array (`pd_linp`) follows immediately after at offset 24.
#[derive(Debug, Clone, Copy)]
pub struct PageHeaderData {
    pub pd_lsn: PageXLogRecPtr,
    pub pd_checksum: u16,
    pub pd_flags: u16,
    pub pd_lower: u16,
    pub pd_upper: u16,
    pub pd_special: u16,
    pub pd_pagesize_version: u16,
    pub pd_prune_xid: u32,
}

impl PageHeaderData {
    pub fn page_size(&self) -> u16 {
        self.pd_pagesize_version & 0xFF00
    }

    pub fn page_version(&self) -> u16 {
        self.pd_pagesize_version & 0x00FF
    }

    /// Number of line pointers on this page.
    pub fn num_line_pointers(&self) -> usize {
        // pd_lower points to end of line pointer array; header is 24 bytes,
        // each line pointer is 4 bytes
        (self.pd_lower as usize).saturating_sub(24) / 4
    }

    /// Parse a `PageHeaderData` from the first 24 bytes of a raw page buffer.
    pub fn parse(page_buffer: &[u8]) -> error::Result<Self> {
        if page_buffer.len() < 24 {
            return Err(error::PgError::PageBufferTooShort {
                actual: page_buffer.len(),
                expected: 24,
            });
        }

        // SAFETY: all slices are within [0, 24) which we validated above.
        // Fixed-size array conversions cannot fail for the exact lengths used.
        let xlogid = u32::from_ne_bytes(page_buffer[0..4].try_into().unwrap());
        let xrecoff = u32::from_ne_bytes(page_buffer[4..8].try_into().unwrap());
        let pd_checksum = u16::from_ne_bytes(page_buffer[8..10].try_into().unwrap());
        let pd_flags = u16::from_ne_bytes(page_buffer[10..12].try_into().unwrap());
        let pd_lower = u16::from_ne_bytes(page_buffer[12..14].try_into().unwrap());
        let pd_upper = u16::from_ne_bytes(page_buffer[14..16].try_into().unwrap());
        let pd_special = u16::from_ne_bytes(page_buffer[16..18].try_into().unwrap());
        let pagesize = u16::from_ne_bytes(page_buffer[18..20].try_into().unwrap()) & 0xFF00;
        let version = u16::from_ne_bytes(page_buffer[18..20].try_into().unwrap()) & 0x00FF;
        let pd_prune_xid = u32::from_ne_bytes(page_buffer[20..24].try_into().unwrap());

        Ok(Self {
            pd_lsn: PageXLogRecPtr { xlogid, xrecoff },
            pd_checksum,
            pd_flags,
            pd_lower,
            pd_upper,
            pd_special,
            pd_pagesize_version: pagesize | version,
            pd_prune_xid,
        })
    }
}

pub const LP_UNUSED: u8 = 0;
pub const LP_NORMAL: u8 = 1;
pub const LP_REDIRECT: u8 = 2;
pub const LP_DEAD: u8 = 3;

#[derive(Clone)]
#[derive_where(Debug)]
pub struct HeapPageData {
    pub header: PageHeaderData,
    pub lp_num: usize,

    #[derive_where(skip)]
    pub lp_items: Vec<ItemIdData>,

    #[derive_where(skip)]
    pub page_data: Bytes,
}

impl HeapPageData {
    /// Parse a full heap page from an owned byte array.
    ///
    /// Copies the buffer into a `Bytes`. Prefer `parse_bytes` when
    /// the caller already has a `Bytes` (e.g. from a bulk read).
    pub fn parse(page_buffer: [u8; PAGE_BUFFER_SIZE]) -> error::Result<Self> {
        Self::parse_bytes(Bytes::copy_from_slice(&page_buffer))
    }

    /// Parse a full heap page from a `Bytes` buffer — zero-copy.
    ///
    /// The `Bytes` is retained for later tuple access (`get_row_data`,
    /// `to_record_batch`). Sub-slicing into it is O(1) with no memcpy.
    pub fn parse_bytes(page_buffer: Bytes) -> error::Result<Self> {
        if page_buffer.len() < PAGE_BUFFER_SIZE {
            return Err(error::PgError::PageBufferTooShort {
                actual: page_buffer.len(),
                expected: PAGE_BUFFER_SIZE,
            });
        }

        let header = PageHeaderData::parse(&page_buffer)?;

        let lp_num = header.num_line_pointers();
        let mut lp_items = Vec::with_capacity(lp_num);
        for lp_index in 0..lp_num {
            lp_items.push(read_line_pointer(&page_buffer, lp_index));
        }

        Ok(Self {
            header,
            lp_num,
            lp_items,
            page_data: page_buffer,
        })
    }

    pub fn get_row_data(&self, ip_posid: u16) -> error::Result<HeapTupleData> {
        let lp = self
            .lp_items
            .get(ip_posid as usize)
            .ok_or(error::PgError::Generic)?;

        log::debug!("lp_item: {}, at index {}", lp, ip_posid);
        if lp.lp_flags() == LP_DEAD {
            return Err(error::PgError::DeadTupleLinePointer {
                ip_posid: ip_posid as usize,
            });
        }
        if lp.lp_flags() == LP_UNUSED {
            return Err(error::PgError::DeadTupleLinePointer {
                ip_posid: ip_posid as usize,
            });
        }
        if lp.lp_flags() == LP_REDIRECT {
            return Err(error::PgError::DeadTupleLinePointer {
                ip_posid: ip_posid as usize,
            });
        }

        let start = lp.lp_off() as usize;
        let end = start
            .checked_add(lp.lp_len() as usize)
            .ok_or(error::PgError::Generic)?;

        let raw = self
            .page_data
            .get(start..end)
            .ok_or(error::PgError::Generic)?;

        HeapTupleData::parse_and_build(raw)
    }

    /// Convert all live tuples on this page directly into an Arrow `RecordBatch`,
    /// bypassing intermediate `PgDatum` / `PgRow` / `HeapTupleData` representations.
    ///
    /// **Zero per-tuple allocation**: tuple headers, null bitmaps, and attribute
    /// data are read as borrowed slices from the page buffer — no `Vec` copies.
    /// Raw bytes are interpreted and appended straight into Arrow column builders.
    ///
    /// If `projection` is `Some`, only those column indices (into `schema`) are
    /// decoded; non-projected columns are skipped by computing byte sizes only.
    /// Pass `None` to include all columns.
    pub fn to_record_batch(
        &self,
        schema: &PgSchema,
        projection: Option<&[usize]>,
    ) -> error::Result<RecordBatch> {
        let num_schema_cols = schema.num_columns();

        // Empty projection → empty batch (zero columns, zero rows).
        if matches!(projection, Some(p) if p.is_empty()) {
            let empty_schema = Arc::new(PgSchema::new(&schema.name, vec![]).to_arrow_schema());
            return RecordBatch::try_new_with_options(
                empty_schema,
                vec![],
                &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(0)),
            )
            .map_err(|e| error::PgError::ArrowConversionFailed {
                detail: e.to_string(),
            });
        }

        // ── Pre-compute column metadata (once, not per-tuple) ───────────
        //
        // Pre-compute both PgTypeId and PgTypeLen for every schema column.
        // This avoids the ~50-arm `type_len()` match on every column of
        // every tuple. For fixed-width columns we also store the size
        // directly so the inner loop can skip the `extract_column_bytes`
        // dispatch entirely.
        let type_ids: Vec<PgTypeId> = (0..num_schema_cols)
            .map(|i| schema.column(i).unwrap().type_id)
            .collect();
        let type_lens: Vec<PgTypeLen> = type_ids.iter().map(|t| t.type_len()).collect();
        // For fixed-width types, pre-compute the size. 0 means "not fixed".
        let fixed_sizes: Vec<usize> = type_lens
            .iter()
            .map(|tl| match tl {
                PgTypeLen::Fixed(n) => *n as usize,
                _ => 0,
            })
            .collect();
        // Pre-compute alignment requirements per column. PostgreSQL aligns
        // each column's start offset to its typalign boundary within the
        // tuple data area (after t_hoff).
        let alignments: Vec<usize> = type_ids.iter().map(|t| t.align()).collect();

        // Determine which columns to output.
        let batch_schema = match projection {
            Some(proj) => schema.project(proj),
            None => schema.clone(),
        };

        // Build a lookup: for each schema column index, what is its output
        // position? `None` means "skip (just advance the offset)".
        // For the common no-projection case, output_pos[i] = Some(i).
        let walk_up_to = match projection {
            Some(proj) => proj.iter().copied().max().map_or(0, |m| m + 1),
            None => num_schema_cols,
        };
        let mut col_output_pos: Vec<Option<usize>> = vec![None; walk_up_to];
        match projection {
            Some(proj) => {
                for (out_idx, &schema_idx) in proj.iter().enumerate() {
                    col_output_pos[schema_idx] = Some(out_idx);
                }
            }
            None => {
                for (i, pos) in col_output_pos[..walk_up_to].iter_mut().enumerate() {
                    *pos = Some(i);
                }
            }
        }

        // Create one ColumnBuilder per output column.
        let num_output_cols = match projection {
            Some(proj) => proj.len(),
            None => num_schema_cols,
        };
        let mut builders: Vec<ColumnBuilder> = Vec::with_capacity(num_output_cols);
        match projection {
            Some(proj) => {
                for &idx in proj {
                    builders.push(ColumnBuilder::for_column(
                        schema.column(idx).unwrap(),
                        self.lp_num,
                    ));
                }
            }
            None => {
                for col in schema.columns() {
                    builders.push(ColumnBuilder::for_column(col, self.lp_num));
                }
            }
        }

        // ── Walk tuples directly from page buffer (zero-copy) ───────────
        for lp_index in 0..self.lp_num {
            let lp = &self.lp_items[lp_index];

            // Skip non-normal line pointers without calling get_row_data.
            if lp.lp_flags() != LP_NORMAL {
                continue;
            }

            let lp_off = lp.lp_off() as usize;
            let lp_len = lp.lp_len() as usize;
            let raw = match self.page_data.get(lp_off..lp_off + lp_len) {
                Some(r) => r,
                None => continue, // corrupted line pointer — skip
            };

            if raw.len() < SIZEOF_HEAP_TUPLE_HEADER {
                continue;
            }

            // Parse t_infomask and t_hoff directly from the raw buffer.
            let t_infomask = u16::from_ne_bytes(raw[20..22].try_into().unwrap());
            let t_infomask2 = u16::from_ne_bytes(raw[18..20].try_into().unwrap());
            let t_hoff = raw[22] as usize;

            if t_hoff < SIZEOF_HEAP_TUPLE_HEADER || t_hoff > raw.len() {
                continue;
            }

            // Borrow null bitmap and attribute data directly from page buffer.
            let has_null = t_infomask & (InfoMask::HasNull as u16) != 0;
            let null_bitmap = if has_null {
                let natts = (t_infomask2 & HEAP_NATTS_MASK) as usize;
                let bitmap_bytes = natts.div_ceil(8);
                let bitmap_end = SIZEOF_HEAP_TUPLE_HEADER + bitmap_bytes;
                if bitmap_end > t_hoff {
                    continue; // corrupt
                }
                &raw[SIZEOF_HEAP_TUPLE_HEADER..bitmap_end]
            } else {
                &[] as &[u8]
            };
            let tuple_data = &raw[t_hoff..];

            // Walk columns, appending to builders.
            let mut data_offset = 0usize;

            for col_index in 0..walk_up_to {
                // Null check: bit=0 means NULL in PostgreSQL.
                let is_null = if has_null {
                    null_bitmap
                        .get(col_index >> 3) // col_index / 8
                        .is_some_and(|b| b & (1u8 << (col_index & 7)) == 0)
                } else {
                    false
                };

                // NULL columns consume no space — skip alignment and data.
                if is_null {
                    if let Some(out_idx) = col_output_pos[col_index] {
                        builders[out_idx].append_null();
                    }
                    continue;
                }

                // Align offset per PostgreSQL's att_align_pointer rules:
                // - Fixed-width: always apply nominal alignment.
                // - Varlena (attlen == -1): peek at the byte at data_offset.
                //   If non-zero, it's the start of a 1-byte varlena header →
                //   no alignment. If zero, it's a pad byte → apply alignment.
                let col_align = alignments[col_index];
                if col_align > 1 {
                    if fixed_sizes[col_index] > 0 {
                        // Fixed-width: always align.
                        data_offset = (data_offset + col_align - 1) & !(col_align - 1);
                    } else if tuple_data.get(data_offset).is_none_or(|&b| b == 0) {
                        // Varlena/cstring: pad byte (0x00) → align to nominal.
                        data_offset = (data_offset + col_align - 1) & !(col_align - 1);
                    }
                    // else: varlena with non-zero first byte → no alignment needed
                }

                let fixed_n = fixed_sizes[col_index];

                match col_output_pos[col_index] {
                    Some(out_idx) => {
                        if fixed_n > 0 {
                            // Fast path: fixed-width — no function call, no type_len dispatch.
                            let (bytes, _) = extract_fixed_bytes(tuple_data, data_offset, fixed_n);
                            builders[out_idx].append_bytes(bytes)?;
                            data_offset += fixed_n;
                        } else {
                            // Variable-length path (varlena/cstring).
                            let (bytes, consumed) = extract_column_bytes(
                                type_lens[col_index],
                                tuple_data,
                                data_offset,
                            )?;
                            builders[out_idx].append_bytes(bytes)?;
                            data_offset += consumed;
                        }
                    }
                    None => {
                        if fixed_n > 0 {
                            // Fixed-width skip: just advance offset, no function call.
                            data_offset += fixed_n;
                        } else {
                            let consumed =
                                skip_datum(type_lens[col_index], tuple_data, data_offset)
                                    .map_err(|e| error::PgError::DecodeError(e.to_string()))?;
                            data_offset += consumed;
                        }
                    }
                }
            }
        }

        // Finish all builders into Arrow arrays.
        let arrays: Vec<_> = builders.into_iter().map(|b| b.finish()).collect();
        let arrow_schema = Arc::new(batch_schema.to_arrow_schema());

        RecordBatch::try_new(arrow_schema, arrays).map_err(|e| {
            error::PgError::ArrowConversionFailed {
                detail: e.to_string(),
            }
        })
    }
}

/// Read the line pointer at `index` from a page buffer.
pub fn read_line_pointer(page: &[u8], index: usize) -> ItemIdData {
    let off = 24 + index * 4;
    ItemIdData(u32::from_ne_bytes(page[off..off + 4].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use arrow::array::{Array, StringArray, UInt32Array};
    use arrow::compute::{filter_record_batch, kernels::cmp::eq};

    use crate::file::reader::TableFileReader;
    use crate::types::{PgAttribute, PgCatalogRelation, PgClass};
    use crate::util::pg_harness;

    #[test]
    fn test_page_to_record_batch_all_columns() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let mut page_reader = reader.get_page_reader().unwrap();
        let page = page_reader.get_page_by_index(0).unwrap();

        let batch = page.to_record_batch(&schema, None).unwrap();

        assert!(batch.num_rows() > 0, "pg_class page 0 should have rows");
        assert_eq!(
            batch.num_columns(),
            schema.num_columns(),
            "batch should have all schema columns"
        );

        // The "relname" column (index 1) should be a StringArray with non-empty values.
        let relname_arr = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("relname column should be StringArray");
        assert!(
            relname_arr.len() > 0 && !relname_arr.value(0).is_empty(),
            "first relname should be non-empty"
        );
    }

    #[test]
    fn test_page_to_record_batch_projected() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let mut page_reader = reader.get_page_reader().unwrap();
        let page = page_reader.get_page_by_index(0).unwrap();

        // Project only: oid (col 0), relname (col 1), relnamespace (col 2)
        let projection = &[0, 1, 2];
        let batch = page.to_record_batch(&schema, Some(projection)).unwrap();

        assert!(batch.num_rows() > 0);
        assert_eq!(
            batch.num_columns(),
            3,
            "projected batch should have 3 columns"
        );

        // Verify schema field names match the projected columns.
        let fields = batch.schema();
        assert_eq!(fields.field(0).name(), "oid");
        assert_eq!(fields.field(1).name(), "relname");
        assert_eq!(fields.field(2).name(), "relnamespace");

        // oid column should have non-zero OIDs.
        let oid_arr = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .expect("oid column should be UInt32Array");
        assert!(oid_arr.value(0) > 0, "first OID should be non-zero");
    }

    #[test]
    fn test_page_to_record_batch_single_column() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let mut page_reader = reader.get_page_reader().unwrap();
        let page = page_reader.get_page_by_index(0).unwrap();

        // Project only relname (col 1) — forces skipping col 0 (oid).
        let batch = page.to_record_batch(&schema, Some(&[1])).unwrap();

        assert_eq!(batch.num_columns(), 1);
        assert_eq!(batch.schema().field(0).name(), "relname");

        let relname_arr = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("relname column should be StringArray");
        assert!(batch.num_rows() > 0);
        for i in 0..relname_arr.len() {
            if !relname_arr.is_null(i) {
                assert!(
                    !relname_arr.value(i).is_empty(),
                    "relname at row {i} should be non-empty"
                );
            }
        }
    }

    #[test]
    fn test_page_to_record_batch_empty_projection() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let mut page_reader = reader.get_page_reader().unwrap();
        let page = page_reader.get_page_by_index(0).unwrap();

        let batch = page.to_record_batch(&schema, Some(&[])).unwrap();
        assert_eq!(batch.num_columns(), 0);
        assert_eq!(batch.num_rows(), 0);
    }

    #[test]
    fn test_parallel_read_all_to_batches() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let page_reader = reader.get_page_reader().unwrap();

        let batches = page_reader.read_all_to_batches(&schema, None).unwrap();

        assert!(!batches.is_empty(), "should produce at least one batch");

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert!(total_rows > 0, "total rows across batches should be > 0");

        // Every batch should have the full schema column count.
        for (i, batch) in batches.iter().enumerate() {
            assert_eq!(
                batch.num_columns(),
                schema.num_columns(),
                "batch {i} should have all schema columns"
            );
        }
    }

    #[test]
    fn test_parallel_read_all_to_batches_projected() {
        let schema = PgClass::catalog_schema();
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let page_reader = reader.get_page_reader().unwrap();

        let projection = &[0, 1]; // oid, relname
        let batches = page_reader
            .read_all_to_batches(&schema, Some(projection))
            .unwrap();

        assert!(!batches.is_empty());
        for batch in &batches {
            assert_eq!(batch.num_columns(), 2);
            assert_eq!(batch.schema().field(0).name(), "oid");
            assert_eq!(batch.schema().field(1).name(), "relname");
        }
    }

    #[test]
    fn test_parallel_matches_sequential() {
        let schema = PgClass::catalog_schema();

        // Sequential: read page 0 directly.
        let reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let mut page_reader = reader.get_page_reader().unwrap();
        let page0 = page_reader.get_page_by_index(0).unwrap();
        let sequential_batch = page0.to_record_batch(&schema, None).unwrap();

        // Parallel: read all, take first batch.
        let reader2 = TableFileReader::new(16384, PgClass::RELATION_OID as usize);
        let page_reader2 = reader2.get_page_reader().unwrap();
        let parallel_batches = page_reader2.read_all_to_batches(&schema, None).unwrap();
        let parallel_first = &parallel_batches[0];

        assert_eq!(
            sequential_batch.num_rows(),
            parallel_first.num_rows(),
            "page 0 row count should match between sequential and parallel"
        );
        assert_eq!(sequential_batch.num_columns(), parallel_first.num_columns(),);

        // Spot-check: compare the relname column (index 1).
        let seq_names = sequential_batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let par_names = parallel_first
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        for i in 0..seq_names.len() {
            assert_eq!(
                seq_names.is_null(i),
                par_names.is_null(i),
                "null mismatch at row {i}"
            );
            if !seq_names.is_null(i) {
                assert_eq!(
                    seq_names.value(i),
                    par_names.value(i),
                    "relname mismatch at row {i}"
                );
            }
        }
    }

    #[test]
    fn test_batch_stream_yields_incrementally() {
        let schema = PgAttribute::catalog_schema();
        let db_id = pg_harness::db_oid_blocking("postgres");
        let reader = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);
        let page_reader = reader.get_page_reader().unwrap();

        // Use a small batch size to force multiple fill_buffer rounds.
        let stream = page_reader
            .into_batch_stream(&schema, None)
            .with_pages_per_batch(1000);

        const FILTER_ATTRELID: u32 = 16728;

        let mut count = 0;
        let mut total_rows = 0;
        let mut filtered_rows = 0;
        for batch_result in stream {
            let batch = batch_result.unwrap();
            assert_eq!(batch.num_columns(), schema.num_columns());
            total_rows += batch.num_rows();
            count += 1;

            // Filter rows where attrelid (col 0) == 16728.
            let attrelid_col = batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .expect("col 0 should be UInt32Array");
            let scalar = UInt32Array::new_scalar(FILTER_ATTRELID);
            let mask = eq(attrelid_col, &scalar).unwrap();
            let filtered = filter_record_batch(&batch, &mask).unwrap();
            filtered_rows += filtered.num_rows();
            if filtered.num_rows() > 0 {
                println!(
                    "batch filtered {}/{} rows for attrelid={FILTER_ATTRELID}",
                    filtered.num_rows(),
                    batch.num_rows()
                );
                let attname_col = filtered
                    .column(1)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .expect("col 1 should be StringArray");
                for i in 0..attname_col.len() {
                    println!("  attname[{i}] = {:?}", attname_col.value(i));
                }
            }
        }
        println!(
            "total_rows={total_rows}, filtered_rows={filtered_rows} (attrelid={FILTER_ATTRELID})"
        );

        assert!(count > 0, "stream should yield at least one batch");
        assert!(total_rows > 0, "stream should yield at least one row");
    }

    #[test]
    fn test_batch_stream_matches_read_all() {
        let schema = PgAttribute::catalog_schema();
        let db_id = pg_harness::db_oid_blocking("postgres");

        // Collect via stream.
        let reader1 = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);
        let stream = reader1
            .get_page_reader()
            .unwrap()
            .into_batch_stream(&schema, None);
        let stream_rows: usize = stream.map(|r| r.unwrap().num_rows()).sum();

        // Collect via read_all.
        let reader2 = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);
        let all_batches = reader2
            .get_page_reader()
            .unwrap()
            .read_all_to_batches(&schema, None)
            .unwrap();
        let all_rows: usize = all_batches.iter().map(|b| b.num_rows()).sum();

        assert_eq!(
            stream_rows, all_rows,
            "stream and read_all should yield the same total rows"
        );
    }

    #[test]
    fn test_page_row_iter_count_matches_batches() {
        let schema = PgAttribute::catalog_schema();
        let db_id = pg_harness::db_oid_blocking("postgres");

        // Count rows via raw HeapTupleData iterator.
        let reader1 = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);
        let row_iter_count: usize = reader1
            .get_page_reader()
            .unwrap()
            .into_iter()
            .filter_map(|r| r.ok())
            .count();

        // Count rows via Arrow batch path.
        let reader2 = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);
        let batch_row_count: usize = reader2
            .get_page_reader()
            .unwrap()
            .read_all_to_batches(&schema, None)
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();

        assert!(
            row_iter_count > 0,
            "row iterator should yield at least one row"
        );
        assert_eq!(
            row_iter_count, batch_row_count,
            "raw row iterator and Arrow batch path should yield the same total rows"
        );
    }
}
