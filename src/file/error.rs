use std::result;

/// Errors produced by pg_arrow file parsing.
#[derive(Debug, thiserror::Error)]
pub enum PgError {
    #[error("generic parse error")]
    Generic,

    /// The raw page buffer is too short to hold a page header.
    #[error("page buffer too short: got {actual} bytes, need at least {expected}")]
    PageBufferTooShort { actual: usize, expected: usize },

    /// A `u16` value did not match any known `InfoMask` variant.
    #[error("unknown infomask value: 0x{0:04X}")]
    UnknownInfoMask(u16),

    /// The raw buffer is too short to hold a heap tuple header.
    /// Minimum required is `SIZEOF_HEAP_TUPLE_HEADER` (23) bytes.
    #[error("tuple buffer too short: got {actual} bytes, need at least {expected}")]
    TupleBufferTooShort { actual: usize, expected: usize },

    /// `t_hoff` is outside the valid range `[SIZEOF_HEAP_TUPLE_HEADER, buf_len]`.
    #[error("invalid t_hoff {t_hoff}: must be in [{min}, {buf_len}]")]
    InvalidTupleHoff {
        t_hoff: u8,
        min: usize,
        buf_len: usize,
    },

    /// The null bitmap (derived from `natts`) extends past `t_hoff`.
    #[error("null bitmap overflows t_hoff: bitmap_end={bitmap_end} > t_hoff={t_hoff}")]
    NullBitmapOverflow { bitmap_end: usize, t_hoff: usize },

    /// DEAD Tuple error
    #[error("the tuple at line pointer index {ip_posid} is dead")]
    DeadTupleLinePointer { ip_posid: usize },

    /// Column is null
    #[error("the column at index {id} is null")]
    NullColumnValue { id: usize },

    /// Column not found by name or index
    #[error("column not found: {column}")]
    ColumnNotFound { column: String },

    /// Datum decoding failed
    #[error("decode error: {0}")]
    DecodeError(String),

    /// The requested table was not found in pg_class
    #[error("table not found: {name}")]
    TableNotFound { name: String },

    /// No table has been selected via `set_table()`
    #[error("no table selected — call set_table() first")]
    NoTableSelected,

    /// Catalog bootstrap (reading pg_class/pg_attribute) failed
    #[error("catalog bootstrap failed: {detail}")]
    CatalogBootstrapFailed { detail: String },

    /// Arrow RecordBatch conversion failed
    #[error("arrow conversion failed: {detail}")]
    ArrowConversionFailed { detail: String },
}

pub type Result<T, E = PgError> = result::Result<T, E>;
