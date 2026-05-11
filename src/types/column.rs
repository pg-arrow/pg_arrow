use arrow::datatypes::Field;

use super::arrow::numeric_typmod_to_arrow_type;
use super::codec::PgTypeId;
use super::pg_type::PgAttribute;

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
