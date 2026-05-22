use std::ffi::CStr;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Decimal256Builder,
    FixedSizeBinaryBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, StringBuilder, Time64MicrosecondBuilder, TimestampMicrosecondBuilder,
    UInt32Builder, UInt64Builder, UInt8Builder,
};
use arrow::datatypes::DataType;
use arrow::datatypes::i256;

use super::codec::{PgTypeId, PgTypeLen, read_varlena_header};
use crate::file::error::{PgError, Result};
use super::PgColumn;

// ────────────────────────────────────────────────────────────────────────────
// PgColumn / PgSchema → Arrow Schema
// ────────────────────────────────────────────────────────────────────────────

/// Decode a PostgreSQL NUMERIC typmod into an Arrow decimal type.
///
/// PostgreSQL encodes `NUMERIC(precision, scale)` as `((precision << 16) | scale) + VARHDRSZ`
/// where `VARHDRSZ = 4`. Unbound NUMERIC has typmod `-1`.
///
/// - precision ≤ 38 → `Decimal128(precision, scale)`
/// - precision > 38 or unbound → `Decimal256(precision, scale)` with defaults `(38, 0)`
pub fn numeric_typmod_to_arrow_type(typmod: i32) -> DataType {
    if typmod > 0 {
        // ((precision << 16) | scale) + 4
        let tm = (typmod - 4) as u32;
        let precision = (tm >> 16) as u8;
        let scale = (tm & 0xFFFF) as i8;
        if precision <= 38 {
            DataType::Decimal128(precision, scale)
        } else {
            DataType::Decimal256(precision, scale)
        }
    } else {
        // Unbound NUMERIC — use Decimal256 with widest safe default
        DataType::Decimal256(38, 0)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PostgreSQL numeric binary → i128 / i256
// ────────────────────────────────────────────────────────────────────────────

/// Each PostgreSQL numeric digit is base 10000.
const NBASE: i128 = 10_000;

// NumericShort header bit masks
const NUMERIC_SHORT_FLAG: u16 = 0x8000;
const NUMERIC_SPECIAL_FLAG: u16 = 0xC000;
const NUMERIC_SIGN_MASK: u16 = 0xC000;
const NUMERIC_SHORT_SIGN_MASK: u16 = 0x2000;
const NUMERIC_SHORT_DSCALE_MASK: u16 = 0x1F80;
const NUMERIC_SHORT_DSCALE_SHIFT: u16 = 7;
const NUMERIC_SHORT_WEIGHT_SIGN_MASK: u16 = 0x0040;
const NUMERIC_SHORT_WEIGHT_MASK: u16 = 0x003F;
// NumericLong sign values (high two bits of n_sign_dscale)
const NUMERIC_NEG_FLAG: u16 = 0x4000;
const NUMERIC_DSCALE_MASK: u16 = 0x3FFF;

/// Parse the PostgreSQL on-disk numeric varlena payload.
///
/// The payload (varlena header already stripped) begins with `NumericChoice`:
/// - **Short form** (`header & 0x8000 != 0`, but `header & 0xC000 != 0xC000`):
///   2-byte header only; sign/dscale/weight packed into `n_header`.
/// - **Long form** (`header & 0xC000 == 0x0000` or `0x4000`):
///   4-byte header: `n_sign_dscale` (u16) + `n_weight` (i16).
/// - **Special** (`header & 0xC000 == 0xC000`): NaN or Infinity → None.
///
/// Returns `(digits_slice, ndigits, weight, dscale, is_negative)`.
fn parse_numeric_header(bytes: &[u8]) -> Option<(&[u8], i32, i32, i32, bool)> {
    if bytes.len() < 2 {
        return None;
    }
    let header = u16::from_ne_bytes(bytes[0..2].try_into().ok()?);

    if header & NUMERIC_SIGN_MASK == NUMERIC_SPECIAL_FLAG {
        // NaN or Infinity
        return None;
    }

    if header & NUMERIC_SHORT_FLAG != 0 {
        // Short format: 2-byte header
        let is_neg = (header & NUMERIC_SHORT_SIGN_MASK) != 0;
        let dscale = ((header & NUMERIC_SHORT_DSCALE_MASK) >> NUMERIC_SHORT_DSCALE_SHIFT) as i32;
        let weight_raw = (header & NUMERIC_SHORT_WEIGHT_MASK) as i32;
        let weight = if header & NUMERIC_SHORT_WEIGHT_SIGN_MASK != 0 {
            weight_raw | !(NUMERIC_SHORT_WEIGHT_MASK as i32)
        } else {
            weight_raw
        };
        let digits = &bytes[2..];
        let ndigits = digits.len() as i32 / 2;
        Some((digits, ndigits, weight, dscale, is_neg))
    } else {
        // Long format: 4-byte header
        if bytes.len() < 4 {
            return None;
        }
        let sign_dscale = u16::from_ne_bytes(bytes[0..2].try_into().ok()?);
        let weight = i16::from_ne_bytes(bytes[2..4].try_into().ok()?) as i32;
        let is_neg = (sign_dscale & NUMERIC_SIGN_MASK) == NUMERIC_NEG_FLAG;
        let dscale = (sign_dscale & NUMERIC_DSCALE_MASK) as i32;
        let digits = &bytes[4..];
        let ndigits = digits.len() as i32 / 2;
        Some((digits, ndigits, weight, dscale, is_neg))
    }
}

/// Decode a PostgreSQL on-disk numeric varlena payload into a scaled i128.
///
/// Returns `(value, scale)` where `value * 10^-scale` represents the number,
/// or `None` for NaN / ±Inf.
pub fn decode_pg_numeric_i128(bytes: &[u8]) -> Option<(i128, i8)> {
    let (digits, ndigits, weight, dscale, is_neg) = parse_numeric_header(bytes)?;

    if ndigits == 0 {
        return Some((0, dscale as i8));
    }

    let mut value: i128 = 0;
    for i in 0..ndigits as usize {
        let d = i16::from_ne_bytes(digits[i * 2..i * 2 + 2].try_into().ok()?) as i128;
        value = value * NBASE + d;
    }

    // value is an integer with (ndigits-1-weight)*4 implicit decimal places.
    // Adjust to dscale.
    let natural_scale = (ndigits - 1 - weight) * 4;
    let diff = dscale - natural_scale;
    if diff > 0 {
        value = value.saturating_mul(10_i128.pow(diff as u32));
    } else if diff < 0 {
        value /= 10_i128.pow((-diff) as u32);
    }

    if is_neg {
        value = -value;
    }

    Some((value, dscale as i8))
}

/// Decode a PostgreSQL on-disk numeric varlena payload into a scaled i256.
pub fn decode_pg_numeric_i256(bytes: &[u8]) -> Option<(i256, i8)> {
    let (digits, ndigits, weight, dscale, is_neg) = parse_numeric_header(bytes)?;

    if ndigits == 0 {
        return Some((i256::ZERO, dscale as i8));
    }

    let nbase256 = i256::from_i128(NBASE);
    let mut value = i256::ZERO;
    for i in 0..ndigits as usize {
        let d = i16::from_ne_bytes(digits[i * 2..i * 2 + 2].try_into().ok()?) as i128;
        value = value.wrapping_mul(nbase256).wrapping_add(i256::from_i128(d));
    }

    let natural_scale = (ndigits - 1 - weight) * 4;
    let diff = dscale - natural_scale;
    if diff > 0 {
        value = value.wrapping_mul(i256::from_i128(10_i128.pow(diff as u32)));
    } else if diff < 0 {
        value = value.wrapping_div(i256::from_i128(10_i128.pow((-diff) as u32)));
    }

    if is_neg {
        value = value.wrapping_neg();
    }

    Some((value, dscale as i8))
}

// ────────────────────────────────────────────────────────────────────────────
// ColumnBuilder — zero-copy bytes → Arrow path
// ────────────────────────────────────────────────────────────────────────────

/// Arrow column builder that accepts raw PostgreSQL on-disk bytes directly,
/// bypassing the intermediate `PgDatum` enum.
///
/// Each variant wraps the corresponding Arrow builder. The [`append_bytes`]
/// method interprets the raw bytes according to the PostgreSQL type and
/// appends the value. [`append_null`] appends a null.
pub enum ColumnBuilder {
    Bool(BooleanBuilder),
    Int2(Int16Builder),
    Int4(Int32Builder),
    Int8(Int64Builder),
    Float4(Float32Builder),
    Float8(Float64Builder),
    Char(UInt8Builder),
    Oid(UInt32Builder),
    Xid(UInt32Builder),
    Cid(UInt32Builder),
    Xid8(UInt64Builder),
    Date(Date32Builder),
    Money(Int64Builder),
    Time(Time64MicrosecondBuilder),
    Timestamp(TimestampMicrosecondBuilder),
    TimestampTz(TimestampMicrosecondBuilder),
    Timetz(Time64MicrosecondBuilder),
    Interval(BinaryBuilder),
    Utf8(StringBuilder),
    Name(StringBuilder),
    Binary(BinaryBuilder),
    FixedBinary(FixedSizeBinaryBuilder),
    Tid(BinaryBuilder),
    /// NUMERIC with precision ≤ 38 — scale stored for decode alignment
    Decimal128(Decimal128Builder, i8),
    /// NUMERIC with precision > 38 or unbound — scale stored for decode alignment
    Decimal256(Decimal256Builder, i8),
}

impl ColumnBuilder {
    /// Create a builder for the given PostgreSQL type, pre-allocated for `capacity` rows.
    /// For NUMERIC columns, use [`ColumnBuilder::for_column`] to preserve typmod.
    pub fn new(type_id: PgTypeId, capacity: usize) -> Self {
        Self::new_inner(type_id, -1, capacity)
    }

    /// Create a builder from a full `PgColumn` descriptor, preserving typmod for NUMERIC.
    pub fn for_column(col: &PgColumn, capacity: usize) -> Self {
        Self::new_inner(col.type_id, col.typmod, capacity)
    }

    fn new_inner(type_id: PgTypeId, typmod: i32, capacity: usize) -> Self {
        match type_id {
            PgTypeId::Bool => Self::Bool(BooleanBuilder::with_capacity(capacity)),
            PgTypeId::Int2 => Self::Int2(Int16Builder::with_capacity(capacity)),
            PgTypeId::Int4 => Self::Int4(Int32Builder::with_capacity(capacity)),
            PgTypeId::Int8 => Self::Int8(Int64Builder::with_capacity(capacity)),
            PgTypeId::Float4 => Self::Float4(Float32Builder::with_capacity(capacity)),
            PgTypeId::Float8 => Self::Float8(Float64Builder::with_capacity(capacity)),
            PgTypeId::Char => Self::Char(UInt8Builder::with_capacity(capacity)),
            PgTypeId::Oid => Self::Oid(UInt32Builder::with_capacity(capacity)),
            PgTypeId::Xid => Self::Xid(UInt32Builder::with_capacity(capacity)),
            PgTypeId::Cid => Self::Cid(UInt32Builder::with_capacity(capacity)),
            PgTypeId::Xid8 => Self::Xid8(UInt64Builder::with_capacity(capacity)),
            PgTypeId::Date => Self::Date(Date32Builder::with_capacity(capacity)),
            PgTypeId::Money => Self::Money(Int64Builder::with_capacity(capacity)),
            PgTypeId::Time => Self::Time(Time64MicrosecondBuilder::with_capacity(capacity)),
            PgTypeId::Timestamp => {
                Self::Timestamp(TimestampMicrosecondBuilder::with_capacity(capacity))
            }
            PgTypeId::Timestamptz => Self::TimestampTz(
                TimestampMicrosecondBuilder::with_capacity(capacity).with_timezone("UTC"),
            ),
            PgTypeId::Timetz => Self::Timetz(Time64MicrosecondBuilder::with_capacity(capacity)),
            PgTypeId::Interval => {
                Self::Interval(BinaryBuilder::with_capacity(capacity, capacity * 16))
            }
            PgTypeId::Name => Self::Name(StringBuilder::with_capacity(capacity, capacity * 64)),
            PgTypeId::Tid => Self::Tid(BinaryBuilder::with_capacity(capacity, capacity * 6)),
            PgTypeId::Text
            | PgTypeId::Varchar
            | PgTypeId::Bpchar
            | PgTypeId::Json
            | PgTypeId::Xml => {
                Self::Utf8(StringBuilder::with_capacity(capacity, capacity * 32))
            }
            PgTypeId::Numeric => {
                let arrow_type = numeric_typmod_to_arrow_type(typmod);
                match arrow_type {
                    DataType::Decimal128(p, s) => {
                        let builder = Decimal128Builder::with_capacity(capacity)
                            .with_data_type(DataType::Decimal128(p, s));
                        Self::Decimal128(builder, s)
                    }
                    DataType::Decimal256(p, s) => {
                        let builder = Decimal256Builder::with_capacity(capacity)
                            .with_data_type(DataType::Decimal256(p, s));
                        Self::Decimal256(builder, s)
                    }
                    _ => unreachable!(),
                }
            }
            // Fixed-size binary: UUID(16), MacAddr(6), MacAddr8(8), Point(16), Line(24),
            // Circle(24), Lseg(32), Box(32)
            PgTypeId::Uuid
            | PgTypeId::Macaddr
            | PgTypeId::Macaddr8
            | PgTypeId::Point
            | PgTypeId::Line
            | PgTypeId::Circle
            | PgTypeId::Lseg
            | PgTypeId::Box => {
                let byte_width = match type_id.type_len() {
                    PgTypeLen::Fixed(n) => n as i32,
                    _ => 16, // fallback
                };
                Self::FixedBinary(FixedSizeBinaryBuilder::with_capacity(capacity, byte_width))
            }
            // Everything else: variable-length binary
            _ => Self::Binary(BinaryBuilder::with_capacity(capacity, capacity * 32)),
        }
    }

    /// Append a null value.
    #[inline]
    pub fn append_null(&mut self) {
        match self {
            Self::Bool(b) => b.append_null(),
            Self::Int2(b) => b.append_null(),
            Self::Int4(b) => b.append_null(),
            Self::Int8(b) => b.append_null(),
            Self::Float4(b) => b.append_null(),
            Self::Float8(b) => b.append_null(),
            Self::Char(b) => b.append_null(),
            Self::Oid(b) => b.append_null(),
            Self::Xid(b) => b.append_null(),
            Self::Cid(b) => b.append_null(),
            Self::Xid8(b) => b.append_null(),
            Self::Date(b) => b.append_null(),
            Self::Money(b) => b.append_null(),
            Self::Time(b) => b.append_null(),
            Self::Timestamp(b) => b.append_null(),
            Self::TimestampTz(b) => b.append_null(),
            Self::Timetz(b) => b.append_null(),
            Self::Interval(b) => b.append_null(),
            Self::Utf8(b) => b.append_null(),
            Self::Name(b) => b.append_null(),
            Self::Binary(b) => b.append_null(),
            Self::FixedBinary(b) => b.append_null(),
            Self::Tid(b) => b.append_null(),
            Self::Decimal128(b, _) => b.append_null(),
            Self::Decimal256(b, _) => b.append_null(),
        }
    }

    /// Interpret raw PostgreSQL bytes and append directly to the Arrow builder.
    ///
    /// `bytes` is the raw column data slice (for fixed-width types) or the
    /// varlena payload (after header stripping, for variable-length types).
    #[inline]
    pub fn append_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Bool(_) => {
                self.append_bool(bytes.first().map(|&b| b != 0).unwrap_or(false));
                Ok(())
            }
            Self::Int2(b) => {
                b.append_value(i16::from_ne_bytes(bytes[..2].try_into().unwrap()));
                Ok(())
            }
            Self::Int4(b) => {
                b.append_value(i32::from_ne_bytes(bytes[..4].try_into().unwrap()));
                Ok(())
            }
            Self::Int8(b) => {
                b.append_value(i64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Float4(b) => {
                b.append_value(f32::from_ne_bytes(bytes[..4].try_into().unwrap()));
                Ok(())
            }
            Self::Float8(b) => {
                b.append_value(f64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Char(b) => {
                b.append_value(bytes[0]);
                Ok(())
            }
            Self::Oid(b) | Self::Xid(b) | Self::Cid(b) => {
                b.append_value(u32::from_ne_bytes(bytes[..4].try_into().unwrap()));
                Ok(())
            }
            Self::Xid8(b) => {
                b.append_value(u64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Date(b) => {
                // PG date: days since 2000-01-01; Arrow Date32: days since 1970-01-01
                let pg_days = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
                b.append_value(pg_days + 10957);
                Ok(())
            }
            Self::Money(b) => {
                b.append_value(i64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Time(b) => {
                b.append_value(i64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Timestamp(b) => {
                // PG timestamp: µs since 2000-01-01; Arrow Timestamp(µs): µs since 1970-01-01
                let pg_us = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
                b.append_value(pg_us + 946_684_800_000_000);
                Ok(())
            }
            Self::TimestampTz(b) => {
                // PG timestamptz: µs since 2000-01-01 UTC; Arrow TimestampTz(µs): µs since 1970-01-01 UTC
                let pg_us = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
                b.append_value(pg_us + 946_684_800_000_000);
                Ok(())
            }
            Self::Timetz(b) => {
                // 12 bytes: 8 bytes time_usec + 4 bytes tz_offset; keep only time
                b.append_value(i64::from_ne_bytes(bytes[..8].try_into().unwrap()));
                Ok(())
            }
            Self::Interval(b) => {
                // PG: 8 bytes usec + 4 days + 4 months → Arrow interval binary: months, days, usec
                // Stack buffer — no heap allocation.
                let mut buf = [0u8; 16];
                buf[..4].copy_from_slice(&bytes[12..16]); // months
                buf[4..8].copy_from_slice(&bytes[8..12]); // days
                buf[8..16].copy_from_slice(&bytes[..8]); // usec
                b.append_value(buf);
                Ok(())
            }
            Self::Name(b) => {
                // Fixed 64-byte null-padded C string
                let s = CStr::from_bytes_until_nul(bytes)
                    .map(|c| c.to_str().unwrap_or(""))
                    .unwrap_or("");
                b.append_value(s);
                Ok(())
            }
            Self::Utf8(b) => {
                // simd-utf8 experiment: validate with NEON, fall back to lossy on error.
                //
                // Microbenchmark (aarch64, Apple M-series, cargo bench --bench simd_utf8):
                // Uses simd_compat — more precise NEON pass, no scalar fallback for valid input;
                // consistently faster than simd_basic at short-medium sizes.
                //
                //   bytes    std (ns)   simd_compat  speedup
                //       8        4.7         5.3      0.89x  ← slight overhead
                //      16        2.7         3.2      0.84x
                //      32        3.5         4.1      0.85x
                //      64        4.5         2.5      1.80x  ← breakeven ~64 bytes
                //     128        5.3         3.1      1.71x
                //     256        7.7         4.4      1.75x
                //     512       12.5         6.2      2.02x
                //    1024       27.2        10.4      2.61x
                //    4096      119.9        35.8      3.35x
                //   16384      508.0       141.4      3.59x
                //   65536     2051.5       540.8      3.79x
                //
                // End-to-end (SF10 lineitem ~60M rows, l_comment/l_shipinstruct/l_shipmode):
                //   no measurable gain — bottleneck is I/O + Arrow buffer building, not validation.
                //
                // Expected win: de-TOASTed large text fields (1KB+), where NEON amortises setup.
                // Re-evaluate once TOAST decompression is implemented.
                #[cfg(feature = "simd-utf8")]
                let s = match simdutf8::compat::from_utf8(bytes) {
                    Ok(s) => std::borrow::Cow::Borrowed(s),
                    // Invalid UTF-8 (e.g. Windows-1251): replace bad bytes with U+FFFD.
                    Err(_) => String::from_utf8_lossy(bytes),
                };
                #[cfg(not(feature = "simd-utf8"))]
                let s = String::from_utf8_lossy(bytes);
                b.append_value(s.as_ref());
                Ok(())
            }
            Self::Decimal128(b, target_scale) => {
                let target_scale = *target_scale;
                match decode_pg_numeric_i128(bytes) {
                    None => b.append_null(),
                    Some((mut val, actual_scale)) => {
                        let diff = target_scale - actual_scale;
                        if diff > 0 {
                            val = val.saturating_mul(10_i128.pow(diff as u32));
                        } else if diff < 0 {
                            val /= 10_i128.pow((-diff) as u32);
                        }
                        b.append_value(val);
                    }
                }
                Ok(())
            }
            Self::Decimal256(b, target_scale) => {
                let target_scale = *target_scale;
                match decode_pg_numeric_i256(bytes) {
                    None => b.append_null(),
                    Some((mut val, actual_scale)) => {
                        let diff = target_scale - actual_scale;
                        if diff > 0 {
                            val = val.wrapping_mul(i256::from_i128(10_i128.pow(diff as u32)));
                        } else if diff < 0 {
                            val = val.wrapping_div(i256::from_i128(10_i128.pow((-diff) as u32)));
                        }
                        b.append_value(val);
                    }
                }
                Ok(())
            }
            Self::Tid(b) => {
                // 6 bytes raw TID
                b.append_value(bytes);
                Ok(())
            }
            Self::FixedBinary(b) => {
                b.append_value(bytes)
                    .map_err(|e| PgError::ArrowConversionFailed {
                        detail: e.to_string(),
                    })?;
                Ok(())
            }
            Self::Binary(b) => {
                b.append_value(bytes);
                Ok(())
            }
        }
    }

    fn append_bool(&mut self, val: bool) {
        if let Self::Bool(b) = self {
            b.append_value(val);
        }
    }

    /// Finish building and return the Arrow array.
    pub fn finish(self) -> ArrayRef {
        match self {
            Self::Bool(mut b) => Arc::new(b.finish()),
            Self::Int2(mut b) => Arc::new(b.finish()),
            Self::Int4(mut b) => Arc::new(b.finish()),
            Self::Int8(mut b) => Arc::new(b.finish()),
            Self::Float4(mut b) => Arc::new(b.finish()),
            Self::Float8(mut b) => Arc::new(b.finish()),
            Self::Char(mut b) => Arc::new(b.finish()),
            Self::Oid(mut b) => Arc::new(b.finish()),
            Self::Xid(mut b) => Arc::new(b.finish()),
            Self::Cid(mut b) => Arc::new(b.finish()),
            Self::Xid8(mut b) => Arc::new(b.finish()),
            Self::Date(mut b) => Arc::new(b.finish()),
            Self::Money(mut b) => Arc::new(b.finish()),
            Self::Time(mut b) => Arc::new(b.finish()),
            Self::Timestamp(mut b) => Arc::new(b.finish()),
            Self::TimestampTz(mut b) => Arc::new(b.finish()),
            Self::Timetz(mut b) => Arc::new(b.finish()),
            Self::Interval(mut b) => Arc::new(b.finish()),
            Self::Utf8(mut b) => Arc::new(b.finish()),
            Self::Name(mut b) => Arc::new(b.finish()),
            Self::Binary(mut b) => Arc::new(b.finish()),
            Self::FixedBinary(mut b) => Arc::new(b.finish()),
            Self::Tid(mut b) => Arc::new(b.finish()),
            Self::Decimal128(mut b, _) => Arc::new(b.finish()),
            Self::Decimal256(mut b, _) => Arc::new(b.finish()),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Byte extractors
// ────────────────────────────────────────────────────────────────────────────

/// Extract the raw byte slice for a column from tuple data.
///
/// For fixed-length types the slice is `data[offset..offset+len]`.
/// For varlena types the slice is the payload after the varlena header.
/// For cstrings the slice includes up to (but not including) the null terminator.
///
/// Returns `(byte_slice, bytes_consumed)`.
#[inline]
pub fn extract_column_bytes(
    type_len: PgTypeLen,
    data: &[u8],
    offset: usize,
) -> Result<(&[u8], usize)> {
    match type_len {
        PgTypeLen::Fixed(n) => {
            let n = n as usize;
            let end = offset + n;
            let bytes = data.get(offset..end).ok_or_else(|| {
                PgError::DecodeError("buffer too short for fixed column".to_owned())
            })?;
            Ok((bytes, n))
        }
        PgTypeLen::Varlena => {
            let (payload_start, payload_len) = read_varlena_header(data, offset)
                .map_err(|e| PgError::DecodeError(e.to_string()))?;
            let payload_end = payload_start + payload_len;
            let payload = data.get(payload_start..payload_end).ok_or_else(|| {
                PgError::DecodeError("buffer too short for varlena payload".to_owned())
            })?;
            let consumed = payload_end - offset;
            Ok((payload, consumed))
        }
        PgTypeLen::CString => {
            let remaining = data
                .get(offset..)
                .ok_or_else(|| PgError::DecodeError("buffer too short for cstring".to_owned()))?;
            let nul_pos = remaining.iter().position(|&b| b == 0).ok_or_else(|| {
                PgError::DecodeError("missing null terminator in cstring".to_owned())
            })?;
            Ok((&remaining[..nul_pos], nul_pos + 1))
        }
    }
}

/// Fast path for fixed-width columns: extract `n` bytes at `offset`.
///
/// Avoids the `type_len()` dispatch when the caller already knows the column
/// is fixed-width. Returns `(byte_slice, n)`.
#[inline(always)]
pub fn extract_fixed_bytes(data: &[u8], offset: usize, n: usize) -> (&[u8], usize) {
    // The caller has validated offset+n is within bounds during page parsing.
    debug_assert!(offset + n <= data.len());
    (&data[offset..offset + n], n)
}
