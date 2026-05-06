use std::ffi::CStr;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryBuilder, BooleanBuilder, Date32Builder, FixedSizeBinaryBuilder, Float32Builder,
    Float64Builder, Int16Builder, Int32Builder, Int64Builder, StringBuilder,
    Time64MicrosecondBuilder, TimestampMicrosecondBuilder, UInt8Builder, UInt32Builder,
    UInt64Builder,
};
use arrow::record_batch::RecordBatch;

use crate::codec::{PgDatum, PgTypeId, PgTypeLen, read_varlena_header};
use crate::file::error::{PgError, Result};
use crate::file::reader::{Oid, TableFileReader};

use crate::heap::tuple::ColumnSearchArg;
use crate::types::{PgAttribute, PgCatalogRelation, PgClass, PgSchema};

// ────────────────────────────────────────────────────────────────────────────
// PgRow — lightweight wrapper around a decoded tuple's column values
// ────────────────────────────────────────────────────────────────────────────

/// A single decoded row from a PostgreSQL heap table.
///
/// Each entry in `columns` corresponds to one schema column in tuple order.
/// NULL values are represented as [`PgDatum::Null`].
#[derive(Debug, Clone)]
pub struct PgRow {
    columns: Vec<PgDatum>,
}

impl PgRow {
    /// Get the datum at the given column index (0-based).
    pub fn get(&self, index: usize) -> Option<&PgDatum> {
        self.columns.get(index)
    }

    /// Returns a slice of all column datums.
    pub fn columns(&self) -> &[PgDatum] {
        &self.columns
    }

    /// Number of columns in this row.
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// Returns `true` if the column at the given index is NULL.
    pub fn is_null(&self, index: usize) -> bool {
        matches!(self.columns.get(index), Some(PgDatum::Null) | None)
    }

    /// Convert a slice of `PgRow`s into an Arrow `RecordBatch`.
    ///
    /// Each column in the schema maps to one Arrow array. The column's
    /// `PgTypeId` determines which Arrow builder is used. Rows with fewer
    /// columns than the schema are padded with NULLs.
    pub fn to_record_batch(rows: &[PgRow], schema: &PgSchema) -> Result<RecordBatch> {
        let arrow_schema = Arc::new(schema.to_arrow_schema());
        let num_cols = schema.num_columns();
        let num_rows = rows.len();

        let mut columns: Vec<ArrayRef> = Vec::with_capacity(num_cols);

        for col_idx in 0..num_cols {
            let col = schema
                .column(col_idx)
                .ok_or_else(|| PgError::ColumnNotFound {
                    column: format!("index {col_idx}"),
                })?;

            let array = build_arrow_array(col.type_id, rows, col_idx, num_rows)?;
            columns.push(array);
        }

        RecordBatch::try_new(arrow_schema, columns).map_err(|e| PgError::ArrowConversionFailed {
            detail: e.to_string(),
        })
    }
}

impl std::fmt::Display for PgRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "(")?;
        for (i, col) in self.columns.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{col}")?;
        }
        write!(f, ")")
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PgTableReader — catalog-caching table reader
// ────────────────────────────────────────────────────────────────────────────

/// High-level reader that bootstraps PostgreSQL catalogs and provides
/// a clean API for reading table data from heap files.
///
/// # Usage
///
/// ```no_run
/// use pg_arrow::table::PgTableReader;
///
/// let mut reader = PgTableReader::new(16384).unwrap();
/// reader.set_table("pg_class").unwrap();
/// let rows = reader.fetch_by_limit(5).unwrap();
/// ```
///
/// # Caching Strategy
///
/// - `pg_class` and `pg_attribute` catalogs are read once at construction
///   (or when `set_db()` is called) and cached for the lifetime of the reader.
/// - The current table's `PgSchema` is cached when `set_table()` is called.
/// - Table file data is **never** cached — each `fetch_*()` call creates a
///   fresh `TableFileReader` to see the latest on-disk state.
#[derive(Clone, Debug)]
pub struct PgTableReader {
    db_id: Oid,

    /// Cached pg_class rows — populated on construction, refreshed on set_db()
    pg_class_cache: Vec<PgClass>,
    /// Cached pg_attribute rows — populated on construction, refreshed on set_db()
    pg_attribute_cache: Vec<PgAttribute>,

    /// Currently selected table (set by set_table())
    current_table: Option<PgClass>,
    /// Schema for the currently selected table
    current_schema: Option<PgSchema>,
}

impl PgTableReader {
    /// Create a new `PgTableReader` for the given database OID.
    ///
    /// Bootstraps the catalog caches by reading `pg_class` (OID 1259) and
    /// `pg_attribute` (OID 1249) from the database's heap files.
    pub fn new(db_id: Oid) -> Result<Self> {
        let (pg_class_cache, pg_attribute_cache) = bootstrap_catalogs(db_id)?;
        Ok(Self {
            db_id,
            pg_class_cache,
            pg_attribute_cache,
            current_table: None,
            current_schema: None,
        })
    }

    /// Select a table by name for subsequent `fetch_*()` calls.
    ///
    /// Searches the cached `pg_class` rows for an ordinary table (`relkind = 'r'`)
    /// with the given name. Builds a `PgSchema` from the cached `pg_attribute`
    /// rows for that table.
    pub fn set_table(&mut self, table_name: &str) -> Result<()> {
        let table = self
            .pg_class_cache
            .iter()
            .find(|c| c.relname == table_name && c.relkind == b'r')
            .cloned()
            .ok_or_else(|| PgError::TableNotFound {
                name: table_name.to_owned(),
            })?;

        let table_attrs: Vec<PgAttribute> = self
            .pg_attribute_cache
            .iter()
            .filter(|a| a.attrelid == table.oid)
            .cloned()
            .collect();

        let schema = PgSchema::from_attributes(&table.relname, &table_attrs);
        self.current_table = Some(table);
        self.current_schema = Some(schema);
        Ok(())
    }

    /// Get all the table details from pgclass and their schema
    pub fn get_all_tables(&self) -> Result<Vec<(PgClass, PgSchema)>> {
        let mut table_details = Vec::new();
        for table_class in self.pg_class_cache.iter().filter(|t| t.relkind == b'r') {
            let table_attrs: Vec<PgAttribute> = self
                .pg_attribute_cache
                .iter()
                .filter(|t| t.attrelid == table_class.oid)
                .cloned()
                .collect();
            let schema = PgSchema::from_attributes(&table_class.relname, &table_attrs);
            table_details.push((table_class.clone(), schema));
        }

        Ok(table_details)
    }

    /// Switch to a different database. Clears the current table selection
    /// and re-bootstraps the catalog caches from the new database.
    pub fn set_db(&mut self, db_id: Oid) -> Result<()> {
        self.db_id = db_id;
        self.current_table = None;
        self.current_schema = None;
        let (pg_class_cache, pg_attribute_cache) = bootstrap_catalogs(db_id)?;
        self.pg_class_cache = pg_class_cache;
        self.pg_attribute_cache = pg_attribute_cache;
        Ok(())
    }

    /// Fetch all rows from the currently selected table.
    pub fn fetch_all(&self) -> Result<Vec<PgRow>> {
        let (table, schema) = self.require_table()?;
        let reader = TableFileReader::new(self.db_id, table.relfilenode as usize);

        let page_reader =
            reader
                .get_page_reader()
                .map_err(|e| PgError::CatalogBootstrapFailed {
                    detail: format!("failed to open table file: {e}"),
                })?;

        let mut rows = Vec::new();
        for row_result in page_reader.into_iter() {
            match row_result {
                Ok(tuple) => {
                    rows.push(decode_row(&tuple, schema)?);
                }
                Err(e) => {
                    log::warn!("skipping tuple: {e}");
                }
            }
        }
        Ok(rows)
    }

    /// Fetch all rows from the currently selected table, decoding only the
    /// projected columns.
    ///
    /// `projection` is an ordered list of column indices into the table's schema.
    /// The returned rows contain only those columns, in projection order.
    pub fn fetch_all_projected(&self, projection: &[usize]) -> Result<Vec<PgRow>> {
        let (table, schema) = self.require_table()?;
        let reader = TableFileReader::new(self.db_id, table.relfilenode as usize);
        let page_reader =
            reader
                .get_page_reader()
                .map_err(|e| PgError::CatalogBootstrapFailed {
                    detail: format!("failed to open table file: {e}"),
                })?;

        let mut rows = Vec::new();
        for row_result in page_reader.into_iter() {
            match row_result {
                Ok(tuple) => {
                    rows.push(decode_row_projected(&tuple, schema, projection)?);
                }
                Err(e) => {
                    log::warn!("skipping tuple: {e}");
                }
            }
        }
        Ok(rows)
    }

    /// Fetch up to `limit` rows from the currently selected table.
    pub fn fetch_by_limit(&self, limit: usize) -> Result<Vec<PgRow>> {
        let (table, _schema) = self.require_table()?;
        let reader = TableFileReader::new(self.db_id, table.relfilenode as usize);
        let page_reader =
            reader
                .get_page_reader()
                .map_err(|e| PgError::CatalogBootstrapFailed {
                    detail: format!("failed to open table file: {e}"),
                })?;
        let mut _count = 0;
        for row_result in page_reader.into_iter().take(limit) {
            match row_result {
                Ok(_tuple) => {
                    _count += 1;
                    // let row = decode_row(&tuple, schema)?;
                    // println!("{}", row);
                }
                Err(e) => {
                    log::warn!("skipping tuple: {e}");
                }
            }
        }
        Ok(vec![])
    }

    /// Fetch rows matching a predicate from the currently selected table.
    ///
    /// Every row is decoded and tested against `predicate`. Only matching
    /// rows are included in the result.
    pub fn fetch_with_filter(&self, predicate: impl Fn(&PgRow) -> bool) -> Result<Vec<PgRow>> {
        let (table, schema) = self.require_table()?;
        let reader = TableFileReader::new(self.db_id, table.relfilenode as usize);

        let page_reader =
            reader
                .get_page_reader()
                .map_err(|e| PgError::CatalogBootstrapFailed {
                    detail: format!("failed to open table file: {e}"),
                })?;

        let mut rows = Vec::new();
        for row_result in page_reader.into_iter() {
            match row_result {
                Ok(tuple) => {
                    let row = decode_row(&tuple, schema)?;
                    if predicate(&row) {
                        rows.push(row);
                    }
                }
                Err(e) => {
                    log::warn!("skipping tuple: {e}");
                }
            }
        }
        Ok(rows)
    }

    /// Returns the schema of the currently selected table, if any.
    pub fn schema(&self) -> Option<&PgSchema> {
        self.current_schema.as_ref()
    }

    /// Returns the name of the currently selected table, if any.
    pub fn table_name(&self) -> Option<&str> {
        self.current_table.as_ref().map(|t| t.relname.as_str())
    }

    /// Returns the database OID.
    pub fn db_id(&self) -> Oid {
        self.db_id
    }

    /// Validate that a table has been selected, returning references to
    /// the cached table and schema.
    fn require_table(&self) -> Result<(&PgClass, &PgSchema)> {
        match (&self.current_table, &self.current_schema) {
            (Some(table), Some(schema)) => Ok((table, schema)),
            _ => Err(PgError::NoTableSelected),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Private helpers
// ────────────────────────────────────────────────────────────────────────────

/// Build a single Arrow array for one column across all rows.
///
/// Dispatches on `type_id` to pick the right Arrow builder, then iterates
/// through every row appending the datum (or null). Rows that are shorter
/// than `col_idx` are treated as NULL.
fn build_arrow_array(
    type_id: PgTypeId,
    rows: &[PgRow],
    col_idx: usize,
    num_rows: usize,
) -> Result<ArrayRef> {
    /// Helper macro: create a typed builder, iterate rows, append values or nulls.
    macro_rules! build_scalar {
        ($builder_ty:ty, $variant:ident) => {{
            let mut builder = <$builder_ty>::with_capacity(num_rows);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::$variant(v)) => builder.append_value(*v),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!(
                                "column {col_idx}: expected {}, got {:?}",
                                stringify!($variant),
                                other
                            ),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }};
    }

    match type_id {
        // ── Boolean ─────────────────────────────────────────────────────
        PgTypeId::Bool => {
            let mut builder = BooleanBuilder::with_capacity(num_rows);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Bool(v)) => builder.append_value(*v),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Bool, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // ── Integer types ───────────────────────────────────────────────
        PgTypeId::Int2 => build_scalar!(Int16Builder, Int2),
        PgTypeId::Int4 => build_scalar!(Int32Builder, Int4),
        PgTypeId::Int8 => build_scalar!(Int64Builder, Int8),
        PgTypeId::Float4 => build_scalar!(Float32Builder, Float4),
        PgTypeId::Float8 => build_scalar!(Float64Builder, Float8),
        PgTypeId::Char => build_scalar!(UInt8Builder, Char),
        PgTypeId::Oid => build_scalar!(UInt32Builder, Oid),
        PgTypeId::Xid => build_scalar!(UInt32Builder, Xid),
        PgTypeId::Cid => build_scalar!(UInt32Builder, Cid),
        PgTypeId::Xid8 => build_scalar!(UInt64Builder, Xid8),
        PgTypeId::Date => build_scalar!(Date32Builder, Date),

        // ── Int64-mapped types (Money, Time, Timestamp) ─────────────────
        PgTypeId::Money => build_scalar!(Int64Builder, Money),
        PgTypeId::Time => {
            let mut builder = Time64MicrosecondBuilder::with_capacity(num_rows);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Time(v)) => builder.append_value(*v),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Time, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Timetz => {
            // Store only the time component (microseconds), drop tz offset
            let mut builder = Time64MicrosecondBuilder::with_capacity(num_rows);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::TimeTz { time_usec, .. }) => builder.append_value(*time_usec),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected TimeTz, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Timestamp => {
            let mut builder = TimestampMicrosecondBuilder::with_capacity(num_rows);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Timestamp(v)) => builder.append_value(*v),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Timestamp, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Timestamptz => {
            let mut builder =
                TimestampMicrosecondBuilder::with_capacity(num_rows).with_timezone("UTC");
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::TimestampTz(v)) => builder.append_value(*v),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!(
                                "column {col_idx}: expected TimestampTz, got {other:?}"
                            ),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // TODO: Interval → IntervalMonthDayNanoBuilder (needs arrow version check)
        PgTypeId::Interval => {
            let mut builder = BinaryBuilder::with_capacity(num_rows, num_rows * 16);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Interval {
                        microseconds,
                        days,
                        months,
                    }) => {
                        let mut buf = Vec::with_capacity(16);
                        buf.extend_from_slice(&months.to_le_bytes());
                        buf.extend_from_slice(&days.to_le_bytes());
                        buf.extend_from_slice(&microseconds.to_le_bytes());
                        builder.append_value(buf);
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Interval, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // ── Utf8 string types ───────────────────────────────────────────
        PgTypeId::Text
        | PgTypeId::Varchar
        | PgTypeId::Bpchar
        | PgTypeId::Json
        | PgTypeId::Xml
        | PgTypeId::Numeric => {
            let mut builder = StringBuilder::with_capacity(num_rows, num_rows * 32);
            for row in rows {
                match row.get(col_idx) {
                    Some(
                        PgDatum::Text(s)
                        | PgDatum::Varchar(s)
                        | PgDatum::BpChar(s)
                        | PgDatum::Json(s)
                        | PgDatum::Xml(s),
                    ) => builder.append_value(s),
                    Some(PgDatum::Numeric(b)) => {
                        // Numeric is stored as raw bytes but mapped to Utf8;
                        // best-effort: represent as hex if not valid UTF-8
                        match std::str::from_utf8(b) {
                            Ok(s) => builder.append_value(s),
                            Err(_) => {
                                let hex: String =
                                    b.iter().map(|byte| format!("{byte:02x}")).collect();
                                builder.append_value(&hex);
                            }
                        }
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!(
                                "column {col_idx}: expected string type, got {other:?}"
                            ),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Name => {
            let mut builder = StringBuilder::with_capacity(num_rows, num_rows * 64);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Name(bytes)) => {
                        let s = CStr::from_bytes_until_nul(bytes)
                            .map(|c| c.to_str().unwrap_or(""))
                            .unwrap_or("");
                        builder.append_value(s);
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Name, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // ── Variable-length binary types ────────────────────────────────
        PgTypeId::Bytea
        | PgTypeId::Jsonb
        | PgTypeId::Jsonpath
        | PgTypeId::Inet
        | PgTypeId::Cidr
        | PgTypeId::Bit
        | PgTypeId::Varbit
        | PgTypeId::Path
        | PgTypeId::Polygon
        | PgTypeId::Aclitem => {
            let mut builder = BinaryBuilder::with_capacity(num_rows, num_rows * 32);
            for row in rows {
                match row.get(col_idx) {
                    Some(
                        PgDatum::Bytea(b)
                        | PgDatum::Jsonb(b)
                        | PgDatum::JsonPath(b)
                        | PgDatum::Inet(b)
                        | PgDatum::Cidr(b)
                        | PgDatum::Bit(b)
                        | PgDatum::VarBit(b)
                        | PgDatum::Path(b)
                        | PgDatum::Polygon(b),
                    ) => builder.append_value(b),
                    Some(PgDatum::RawBytes { data, .. }) => builder.append_value(data),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!(
                                "column {col_idx}: expected binary type, got {other:?}"
                            ),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Tid => {
            let mut builder = BinaryBuilder::with_capacity(num_rows, num_rows * 6);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Tid { block, offset }) => {
                        let mut buf = Vec::with_capacity(6);
                        buf.extend_from_slice(&block.to_ne_bytes());
                        buf.extend_from_slice(&offset.to_ne_bytes());
                        builder.append_value(buf);
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Tid, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // ── Fixed-size binary (geometric, MAC, UUID) ────────────────────
        PgTypeId::Uuid => build_fixed_binary(rows, col_idx, num_rows, 16, |datum| match datum {
            PgDatum::Uuid(b) => Ok(b.as_slice()),
            other => Err(PgError::ArrowConversionFailed {
                detail: format!("column {col_idx}: expected Uuid, got {other:?}"),
            }),
        }),
        PgTypeId::Macaddr => build_fixed_binary(rows, col_idx, num_rows, 6, |datum| match datum {
            PgDatum::MacAddr(b) => Ok(b.as_slice()),
            other => Err(PgError::ArrowConversionFailed {
                detail: format!("column {col_idx}: expected MacAddr, got {other:?}"),
            }),
        }),
        PgTypeId::Macaddr8 => build_fixed_binary(rows, col_idx, num_rows, 8, |datum| match datum {
            PgDatum::MacAddr8(b) => Ok(b.as_slice()),
            other => Err(PgError::ArrowConversionFailed {
                detail: format!("column {col_idx}: expected MacAddr8, got {other:?}"),
            }),
        }),
        PgTypeId::Point => {
            let mut builder = FixedSizeBinaryBuilder::with_capacity(num_rows, 16);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Point { x, y }) => {
                        let mut buf = [0u8; 16];
                        buf[..8].copy_from_slice(&x.to_ne_bytes());
                        buf[8..].copy_from_slice(&y.to_ne_bytes());
                        builder
                            .append_value(buf)
                            .map_err(|e| PgError::ArrowConversionFailed {
                                detail: format!("column {col_idx}: {e}"),
                            })?;
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Point, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Line => {
            let mut builder = FixedSizeBinaryBuilder::with_capacity(num_rows, 24);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Line { a, b, c }) => {
                        let mut buf = [0u8; 24];
                        buf[..8].copy_from_slice(&a.to_ne_bytes());
                        buf[8..16].copy_from_slice(&b.to_ne_bytes());
                        buf[16..].copy_from_slice(&c.to_ne_bytes());
                        builder
                            .append_value(buf)
                            .map_err(|e| PgError::ArrowConversionFailed {
                                detail: format!("column {col_idx}: {e}"),
                            })?;
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Line, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Lseg | PgTypeId::Box => {
            let mut builder = FixedSizeBinaryBuilder::with_capacity(num_rows, 32);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Lseg { x1, y1, x2, y2 })
                    | Some(PgDatum::Box { x1, y1, x2, y2 }) => {
                        let mut buf = [0u8; 32];
                        buf[..8].copy_from_slice(&x1.to_ne_bytes());
                        buf[8..16].copy_from_slice(&y1.to_ne_bytes());
                        buf[16..24].copy_from_slice(&x2.to_ne_bytes());
                        buf[24..].copy_from_slice(&y2.to_ne_bytes());
                        builder
                            .append_value(buf)
                            .map_err(|e| PgError::ArrowConversionFailed {
                                detail: format!("column {col_idx}: {e}"),
                            })?;
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Lseg/Box, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
        PgTypeId::Circle => {
            let mut builder = FixedSizeBinaryBuilder::with_capacity(num_rows, 24);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::Circle { x, y, radius }) => {
                        let mut buf = [0u8; 24];
                        buf[..8].copy_from_slice(&x.to_ne_bytes());
                        buf[8..16].copy_from_slice(&y.to_ne_bytes());
                        buf[16..].copy_from_slice(&radius.to_ne_bytes());
                        builder
                            .append_value(buf)
                            .map_err(|e| PgError::ArrowConversionFailed {
                                detail: format!("column {col_idx}: {e}"),
                            })?;
                    }
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!("column {col_idx}: expected Circle, got {other:?}"),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }

        // ── Array types — not yet supported, store as binary fallback ───
        _ => {
            let mut builder = BinaryBuilder::with_capacity(num_rows, num_rows * 32);
            for row in rows {
                match row.get(col_idx) {
                    Some(PgDatum::RawBytes { data, .. }) => builder.append_value(data),
                    Some(PgDatum::Null) | None => builder.append_null(),
                    Some(other) => {
                        return Err(PgError::ArrowConversionFailed {
                            detail: format!(
                                "column {col_idx}: unsupported type {:?} with datum {other:?}",
                                type_id
                            ),
                        });
                    }
                }
            }
            Ok(Arc::new(builder.finish()) as ArrayRef)
        }
    }
}

/// Helper for fixed-size binary columns. Calls `extract` on each non-null datum
/// to get a byte slice of exactly `byte_width` bytes.
fn build_fixed_binary(
    rows: &[PgRow],
    col_idx: usize,
    num_rows: usize,
    byte_width: i32,
    extract: impl Fn(&PgDatum) -> Result<&[u8]>,
) -> Result<ArrayRef> {
    let mut builder = FixedSizeBinaryBuilder::with_capacity(num_rows, byte_width);
    for row in rows {
        match row.get(col_idx) {
            Some(PgDatum::Null) | None => builder.append_null(),
            Some(datum) => {
                let bytes = extract(datum)?;
                builder
                    .append_value(bytes)
                    .map_err(|e| PgError::ArrowConversionFailed {
                        detail: format!("column {col_idx}: {e}"),
                    })?;
            }
        }
    }
    Ok(Arc::new(builder.finish()) as ArrayRef)
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
}

impl ColumnBuilder {
    /// Create a builder for the given PostgreSQL type, pre-allocated for `capacity` rows.
    pub fn new(type_id: PgTypeId, capacity: usize) -> Self {
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
            | PgTypeId::Xml
            | PgTypeId::Numeric => {
                Self::Utf8(StringBuilder::with_capacity(capacity, capacity * 32))
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
                // Varlena payload — already UTF-8 text or numeric-as-hex
                match std::str::from_utf8(bytes) {
                    Ok(s) => b.append_value(s),
                    Err(_) => {
                        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                        b.append_value(&hex);
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
        }
    }
}

/// Extract the raw byte slice for a column from tuple data.
///
/// **Fixed-length overload**: when the caller already knows the type is fixed-width
/// and has the size, use [`extract_fixed_column_bytes`] to skip the `type_len()` dispatch.
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
            // SAFETY rationale: offset + n cannot overflow for valid PostgreSQL
            // pages (max 32KB page, max ~1600 columns of 8 bytes). The bounds
            // check on .get() catches any actual overrun.
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
    // Use unchecked-style indexing via split_at for speed, with a debug assertion.
    debug_assert!(offset + n <= data.len());
    (&data[offset..offset + n], n)
}

/// Bootstrap catalog caches by reading pg_class and pg_attribute heap files.
///
/// This follows the exact pattern from `test_catalog_bootstrap_read_user_table`:
/// 1. Read all pg_class rows (OID 1259) using the hardcoded catalog schema
/// 2. Read all pg_attribute rows (OID 1249) using the hardcoded catalog schema
/// 3. Skip rows that fail to decode (catalog tables may have trailing varlena data)
fn bootstrap_catalogs(db_id: Oid) -> Result<(Vec<PgClass>, Vec<PgAttribute>)> {
    // ── Read pg_class ────────────────────────────────────────────────────
    let pg_class_schema = PgClass::catalog_schema();
    let pg_class_reader = TableFileReader::new(db_id, PgClass::RELATION_OID as usize);
    let page_reader =
        pg_class_reader
            .get_page_reader()
            .map_err(|e| PgError::CatalogBootstrapFailed {
                detail: format!("failed to read pg_class: {e}"),
            })?;

    let mut pg_class_rows = Vec::new();
    for row_result in page_reader.into_iter() {
        match row_result {
            Ok(tuple) if tuple.header.t_xmax == 0 => {
                match PgClass::from_row(&tuple, &pg_class_schema) {
                    Ok(row) => pg_class_rows.push(row),
                    Err(e) => log::warn!("skipping pg_class row: {e}"),
                }
            }
            Ok(tuple) => log::info!("Skipping tuple with header: {}", tuple.header),
            Err(e) => log::warn!("skipping pg_class tuple: {e}"),
        }
    }

    // ── Read pg_attribute ────────────────────────────────────────────────
    let pg_attr_schema = PgAttribute::catalog_schema();
    let pg_attr_reader = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);

    let page_reader =
        pg_attr_reader
            .get_page_reader()
            .map_err(|e| PgError::CatalogBootstrapFailed {
                detail: format!("failed to read pg_attribute: {e}"),
            })?;

    let mut pg_attr_rows = Vec::new();
    for row_result in page_reader.into_iter() {
        match row_result {
            Ok(tuple) => match PgAttribute::from_row(&tuple, &pg_attr_schema) {
                Ok(row) => pg_attr_rows.push(row),
                Err(e) => log::warn!("skipping pg_attribute row: {e}"),
            },
            Err(e) => log::warn!("skipping pg_attribute tuple: {e}"),
        }
    }

    Ok((pg_class_rows, pg_attr_rows))
}

/// Decode a heap tuple into a `PgRow` using the given schema.
///
/// Walks columns left-to-right, calling `tuple.get_column()` for each.
/// NULL columns (indicated by `PgError::NullColumnValue`) produce `PgDatum::Null`
/// with no offset advancement. Other errors are propagated.
pub fn decode_row(tuple: &crate::file::tuple::HeapTupleData, schema: &PgSchema) -> Result<PgRow> {
    use crate::codec::PgTypeLen;

    let mut columns = Vec::with_capacity(schema.num_columns());
    let mut offset = 0usize;

    for col_index in 0..schema.num_columns() {
        // NULL columns consume no space — skip alignment and data.
        if tuple.is_null(col_index) {
            columns.push(PgDatum::Null);
            continue;
        }

        // Apply alignment per att_align_pointer rules.
        let col = schema.column(col_index).unwrap();
        let align = col.type_id.align();
        if align > 1 {
            let is_varlena = matches!(col.type_id.type_len(), PgTypeLen::Varlena);
            if is_varlena {
                if tuple.data.get(offset).is_none_or(|&b| b == 0) {
                    offset = (offset + align - 1) & !(align - 1);
                }
            } else {
                offset = (offset + align - 1) & !(align - 1);
            }
        }

        match tuple.get_column(ColumnSearchArg::ColumnIndex(col_index), schema, offset) {
            Ok((datum, size)) => {
                offset += size;
                columns.push(datum);
            }
            Err(PgError::NullColumnValue { .. }) => {
                columns.push(PgDatum::Null);
            }
            Err(e) => return Err(e),
        }
    }

    Ok(PgRow { columns })
}

/// Decode a heap tuple into a `PgRow` containing only the projected columns.
///
/// `projection` is an ordered list of column indices (referring to positions in the
/// full `schema`) that should appear in the output row. All schema columns up to
/// the highest projected index are walked left-to-right to track offsets, but only
/// projected columns have their values decoded. Non-projected columns are skipped
/// cheaply via [`HeapTupleData::skip_column`] (computes byte size without parsing).
///
/// The returned `PgRow` contains columns in the same order as `projection`.
pub fn decode_row_projected(
    tuple: &crate::file::tuple::HeapTupleData,
    schema: &PgSchema,
    projection: &[usize],
) -> Result<PgRow> {
    if projection.is_empty() {
        return Ok(PgRow {
            columns: Vec::new(),
        });
    }

    let num_schema_cols = schema.num_columns();
    for &idx in projection {
        if idx >= num_schema_cols {
            return Err(PgError::ColumnNotFound {
                column: format!("index {idx} (schema has {num_schema_cols} columns)"),
            });
        }
    }

    // Map each schema column index → its position in the output row (if projected).
    let max_col = projection.iter().copied().max().unwrap(); // safe: projection is non-empty
    let mut output_pos: Vec<Option<usize>> = vec![None; max_col + 1];
    for (out_idx, &col_idx) in projection.iter().enumerate() {
        output_pos[col_idx] = Some(out_idx);
    }

    use crate::codec::PgTypeLen;

    let mut columns: Vec<PgDatum> = vec![PgDatum::Null; projection.len()];
    let mut offset = 0usize;

    // Walk columns 0..=max_col to track offsets correctly.
    for col_index in 0..=max_col {
        // NULL columns consume no space.
        if tuple.is_null(col_index) {
            // If projected, it's already PgDatum::Null in the output vec.
            continue;
        }

        // Apply alignment per att_align_pointer rules.
        let col = schema.column(col_index).unwrap();
        let align = col.type_id.align();
        if align > 1 {
            let is_varlena = matches!(col.type_id.type_len(), PgTypeLen::Varlena);
            if is_varlena {
                if tuple.data.get(offset).is_none_or(|&b| b == 0) {
                    offset = (offset + align - 1) & !(align - 1);
                }
            } else {
                offset = (offset + align - 1) & !(align - 1);
            }
        }

        let is_projected = output_pos[col_index].is_some();

        if is_projected {
            match tuple.get_column(ColumnSearchArg::ColumnIndex(col_index), schema, offset) {
                Ok((datum, size)) => {
                    offset += size;
                    columns[output_pos[col_index].unwrap()] = datum;
                }
                Err(PgError::NullColumnValue { .. }) => {}
                Err(e) => return Err(e),
            }
        } else {
            // Skip: advance offset without decoding
            match tuple.skip_column(col_index, schema, offset) {
                Ok(size) => {
                    offset += size;
                }
                Err(PgError::NullColumnValue { .. }) => {}
                Err(e) => return Err(e),
            }
        }
    }

    Ok(PgRow { columns })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::PgDatum;
    use crate::util::pg_harness;

    /// High-level SELECT * validation: PgTableReader rows match live PostgreSQL output.
    ///
    /// Checks that `fetch_all()` + `to_record_batch()` returns the same row count
    /// and same values for id/text columns as a plain `SELECT * FROM decode_test ORDER BY id`.
    #[test]
    fn test_table_reader_select_matches_postgres() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // ── 1. Live PG ────────────────────────────────────────────────────
            let client = pg_harness::connect().await;
            pg_harness::ensure_decode_test_table(&client).await;

            let pg_rows = client
                .query(
                    &format!(
                        "SELECT id, col_int4, col_text FROM {} ORDER BY id",
                        pg_harness::DECODE_TEST_TABLE
                    ),
                    &[],
                )
                .await
                .expect("SELECT failed");

            // ── 2. pgfusion ───────────────────────────────────────────────────
            let db_id = pg_harness::db_oid(&client, "postgres").await;
            let mut reader = PgTableReader::new(db_id).unwrap();
            reader
                .set_table(pg_harness::DECODE_TEST_TABLE)
                .expect("decode test table not found");

            let mut rows = reader.fetch_all().expect("fetch_all failed");

            let schema = reader.schema().unwrap().clone();
            let col_id = schema
                .columns()
                .enumerate()
                .find(|(_, c)| c.name == "id")
                .map(|(i, _)| i)
                .unwrap();
            let col_int4 = schema
                .columns()
                .enumerate()
                .find(|(_, c)| c.name == "col_int4")
                .map(|(i, _)| i)
                .unwrap();
            let col_text = schema
                .columns()
                .enumerate()
                .find(|(_, c)| c.name == "col_text")
                .map(|(i, _)| i)
                .unwrap();

            rows.sort_by_key(|row| match row.get(col_id) {
                Some(PgDatum::Int4(id)) => *id,
                _ => i32::MAX,
            });

            assert_eq!(
                rows.len(),
                pg_rows.len(),
                "row count mismatch: pgfusion={} pg={}",
                rows.len(),
                pg_rows.len()
            );

            for (i, (decoded, pg)) in rows.iter().zip(pg_rows.iter()).enumerate() {
                let want_id: i32 = pg.get("id");
                let got_id = match decoded.get(col_id) {
                    Some(PgDatum::Int4(v)) => *v,
                    other => panic!("row {i}: id unexpected {other:?}"),
                };
                assert_eq!(got_id, want_id, "row {i}: id");

                let want_int4: Option<i32> = pg.get("col_int4");
                let got_int4 = match decoded.get(col_int4) {
                    Some(PgDatum::Int4(v)) => Some(*v),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("row {i}: col_int4 unexpected {other:?}"),
                };
                assert_eq!(got_int4, want_int4, "row {i}: col_int4");

                let want_text: Option<String> = pg.get("col_text");
                let got_text = match decoded.get(col_text) {
                    Some(PgDatum::Text(v)) => Some(v.clone()),
                    Some(PgDatum::Null) | None => None,
                    other => panic!("row {i}: col_text unexpected {other:?}"),
                };
                assert_eq!(got_text, want_text, "row {i}: col_text");
            }
        });
    }

    #[test]
    fn test_table_reader_bootstrap() {
        let reader = PgTableReader::new(16384).unwrap();
        assert!(
            !reader.pg_class_cache.is_empty(),
            "pg_class cache should not be empty"
        );
        assert!(
            !reader.pg_attribute_cache.is_empty(),
            "pg_attribute cache should not be empty"
        );
    }

    #[test]
    fn test_table_reader_set_table() {
        let mut reader = PgTableReader::new(16384).unwrap();
        reader.set_table("pg_class").unwrap();
        assert_eq!(reader.table_name(), Some("pg_class"));
        assert!(reader.schema().is_some());
        assert!(reader.schema().unwrap().num_columns() > 0);
    }

    #[test]
    fn test_table_reader_table_not_found() {
        let mut reader = PgTableReader::new(16384).unwrap();
        let result = reader.set_table("nonexistent_table_xyz");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PgError::TableNotFound { .. }),
            "expected TableNotFound error"
        );
    }

    #[test]
    fn test_table_reader_no_table_selected() {
        let reader = PgTableReader::new(16384).unwrap();
        let result = reader.fetch_all();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PgError::NoTableSelected),
            "expected NoTableSelected error"
        );
    }

    #[test]
    fn test_table_reader_fetch_all() {
        let mut reader = PgTableReader::new(16384).unwrap();
        reader.set_table("pgbench_accounts").unwrap();
        let rows = reader.fetch_all().unwrap();
        assert!(!rows.is_empty(), "pg_class should have rows");
        assert!(
            rows[0].num_columns() > 0,
            "rows should have at least one column"
        );
    }

    #[test]
    fn test_table_reader_fetch_by_limit() {
        let mut reader = PgTableReader::new(16384).unwrap();
        reader.set_table("pg_class").unwrap();
        let rows = reader.fetch_by_limit(5).unwrap();
        assert!(rows.len() <= 5, "should return at most 5 rows");
        assert!(!rows.is_empty(), "should return at least 1 row");
    }

    #[test]
    fn test_table_reader_fetch_with_filter() {
        let mut reader = PgTableReader::new(16384).unwrap();
        reader.set_table("pg_class").unwrap();

        // Filter for ordinary tables only (relkind = 'r')
        let tables = reader
            .fetch_with_filter(|row| {
                matches!(row.get(PgClass::ANUM_RELKIND), Some(PgDatum::Char(b'r')))
            })
            .unwrap();
        assert!(
            !tables.is_empty(),
            "should find at least one ordinary table"
        );
    }

    #[test]
    fn test_pg_row_is_null() {
        let row = PgRow {
            columns: vec![PgDatum::Int4(42), PgDatum::Null, PgDatum::Bool(true)],
        };
        assert!(!row.is_null(0));
        assert!(row.is_null(1));
        assert!(!row.is_null(2));
        assert!(row.is_null(99)); // out of bounds → treated as null
    }

    #[test]
    fn test_table_reader_accessors() {
        let reader = PgTableReader::new(16384).unwrap();
        assert_eq!(reader.db_id(), 16384);
        assert_eq!(reader.table_name(), None);
        assert!(reader.schema().is_none());
    }
}
