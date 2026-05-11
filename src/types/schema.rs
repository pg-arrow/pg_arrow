use arrow::datatypes::{Field, Schema};

use super::column::PgColumn;
use super::pg_type::PgAttribute;

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
