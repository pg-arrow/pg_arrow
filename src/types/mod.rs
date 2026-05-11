pub mod pg_type;

pub use pg_type::{PgAttribute, PgClass, PgProc, PgType};

use crate::codec::PgTypeId;
use arrow::datatypes::{DataType, Field, Schema};

/// Trait for PostgreSQL system catalog relations.
///
/// Each system catalog (pg_attribute, pg_class, pg_type, pg_proc, etc.) has a
/// fixed OID, a known set of fixed-width attribute types, and corresponding
/// attribute names. This trait captures that compile-time metadata and provides
/// a default `catalog_schema()` method to build a [`PgSchema`] without needing
/// to read `pg_attribute` rows from disk.
pub trait PgCatalogRelation {
    /// The relation OID from pg_class (e.g., 1249 for pg_attribute).
    const RELATION_OID: u32;
    /// Number of fixed-width attributes (excludes trailing varlena columns).
    const NUM_FIXED_ATTRS: usize;

    /// Type OIDs for each attribute, in tuple order.
    fn attr_types() -> &'static [PgTypeId];
    /// Column names for each attribute, in tuple order (matching PostgreSQL field names).
    fn attr_names() -> &'static [&'static str];
    /// Human-readable catalog name (e.g., "pg_attribute").
    fn catalog_name() -> &'static str;

    /// Build a [`PgSchema`] from compile-time catalog metadata.
    ///
    /// All columns are marked nullable since system catalog columns can
    /// technically be NULL (though most are NOT NULL in practice).
    fn catalog_schema() -> PgSchema {
        let columns = Self::attr_names()
            .iter()
            .zip(Self::attr_types().iter())
            .map(|(name, type_id)| PgColumn::new(*name, *type_id, true, -1))
            .collect();
        PgSchema::new(Self::catalog_name(), columns)
    }
}

/// A single column in a PostgreSQL relation, carrying the minimal metadata
/// needed for tuple decoding and Arrow conversion.
#[derive(Debug, Clone)]
pub struct PgColumn {
    /// Column name (from pg_attribute.attname)
    pub name: String,
    /// Type OID (from pg_attribute.atttypid)
    pub type_id: PgTypeId,
    /// Whether the column allows NULLs (inverse of pg_attribute.attnotnull)
    pub nullable: bool,
    /// Type modifier (atttypmod): encodes precision/scale for NUMERIC, length for VARCHAR, etc.
    /// -1 means "no modifier" (unbound/unspecified).
    pub typmod: i32,
}

impl PgColumn {
    /// Create a new column descriptor.
    pub fn new(name: impl Into<String>, type_id: PgTypeId, nullable: bool, typmod: i32) -> Self {
        Self {
            name: name.into(),
            type_id,
            nullable,
            typmod,
        }
    }

    /// Convert to an Arrow `Field`.
    pub fn to_arrow_field(&self) -> Field {
        let dt = if self.type_id == PgTypeId::Numeric {
            numeric_typmod_to_arrow_type(self.typmod)
        } else {
            self.type_id.arrow_type()
        };
        Field::new(&self.name, dt, self.nullable)
    }
}

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

impl From<&PgAttribute> for PgColumn {
    /// Build a `PgColumn` from a decoded `PgAttribute` row.
    ///
    /// Uses `atttypid` to look up the `PgTypeId`, and derives nullability
    /// from `attnotnull`. The column name is extracted from the fixed-size
    /// `attname` byte array (NUL-padded NAMEDATALEN).
    fn from(attr: &PgAttribute) -> Self {
        let type_id = PgTypeId::try_from(attr.atttypid).unwrap_or(PgTypeId::Text);

        Self {
            name: attr.attname.to_owned(),
            type_id,
            nullable: !attr.attnotnull,
            typmod: attr.atttypmod,
        }
    }
}

/// Schema of a PostgreSQL relation — an ordered list of columns.
///
/// Column order matches the on-disk tuple layout (by `attnum`), so the
/// decoder can walk columns left-to-right using index offsets.
#[derive(Debug, Clone)]
pub struct PgSchema {
    /// Relation name
    pub name: String,
    /// Columns in tuple order
    columns: Vec<PgColumn>,
}

impl PgSchema {
    /// Create a new schema with the given name and columns.
    pub fn new(name: impl Into<String>, columns: Vec<PgColumn>) -> Self {
        Self {
            name: name.into(),
            columns,
        }
    }

    /// Build a schema from a slice of `PgAttribute` rows.
    ///
    /// Filters out dropped columns (`attisdropped`) and system columns
    /// (`attnum <= 0`), then sorts by `attnum` to guarantee tuple order.
    pub fn from_attributes(name: impl Into<String>, attrs: &[PgAttribute]) -> Self {
        let mut user_attrs: Vec<&PgAttribute> = attrs
            .iter()
            .filter(|a| !a.attisdropped && a.attnum > 0)
            .collect();
        user_attrs.sort_by_key(|a| a.attnum);

        let columns = user_attrs.iter().map(|a| PgColumn::from(*a)).collect();

        Self {
            name: name.into(),
            columns,
        }
    }

    /// Convert to an Arrow `Schema`.
    pub fn to_arrow_schema(&self) -> Schema {
        let fields: Vec<Field> = self.columns.iter().map(|c| c.to_arrow_field()).collect();
        Schema::new(fields)
    }

    /// Number of columns.
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// Get a column by index (0-based, in tuple order).
    pub fn column(&self, index: usize) -> Option<&PgColumn> {
        self.columns.get(index)
    }

    /// Get a column by name (linear scan).
    pub fn column_by_name(&self, name: &str) -> Option<&PgColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Iterator over columns in tuple order.
    pub fn columns(&self) -> impl Iterator<Item = &PgColumn> {
        self.columns.iter()
    }

    /// Create a sub-schema containing only the specified columns, in the given order.
    ///
    /// `indices` refers to column positions in this schema (0-based, tuple order).
    /// The returned schema preserves the order of `indices`, so `project(&[2, 0])`
    /// yields a schema with the original column 2 first, then column 0.
    ///
    /// # Panics
    ///
    /// Panics if any index is out of bounds.
    pub fn project(&self, indices: &[usize]) -> PgSchema {
        let columns = indices
            .iter()
            .map(|&i| self.columns[i].clone())
            .collect();
        PgSchema::new(&self.name, columns)
    }
}

#[cfg(test)]
mod tests {

    use arrow::datatypes::{DataType, Field, Fields, Schema};

    #[test]
    fn test_arrow_schema() {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new(
                "nested",
                DataType::Struct(Fields::from(vec![
                    Field::new("a", DataType::Utf8, false),
                    Field::new("b", DataType::Float64, false),
                    Field::new("c", DataType::Float64, false),
                ])),
                false,
            ),
        ]);

        println!("{:?}", schema.fields());
    }
}
