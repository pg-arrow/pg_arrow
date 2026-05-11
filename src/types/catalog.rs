use super::codec::PgTypeId;
use super::{PgColumn, PgSchema};

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
