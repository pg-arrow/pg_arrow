/// Rust representation of PostgreSQL's `pg_class` system catalog (OID 1259).
///
/// This mirrors the fixed-length portion of `CATALOG(pg_class)` from
/// `src/include/catalog/pg_class.h`. Variable-length fields (`relacl`,
/// `reloptions`, `relpartbound`) are not included — they live past the
/// null bitmap and require varlena decoding.
///
/// Field order matches the on-disk tuple layout in the `pg_class` heap.
#[derive(Debug, Clone)]
pub struct PgClass {
    /// pg_class.oid — relation object identifier
    pub oid: u32,
    /// Class name
    pub relname: String,
    /// OID of namespace (schema) containing this relation
    pub relnamespace: u32,
    /// OID of entry in pg_type for this relation's implicit row type
    pub reltype: u32,
    /// OID of pg_type entry for underlying composite type (0 if none)
    pub reloftype: u32,
    /// Relation owner (references pg_authid.oid)
    pub relowner: u32,
    /// Access method OID; 0 if not a table/index (references pg_am.oid)
    pub relam: u32,
    /// Physical storage filenode; 0 means "mapped" (see relmapper.c)
    pub relfilenode: u32,
    /// Tablespace OID; 0 means default for database
    pub reltablespace: u32,
    /// Estimated number of disk blocks (not always up-to-date)
    pub relpages: i32,
    /// Estimated number of live rows; -1 means unknown
    pub reltuples: f32,
    /// Number of all-visible blocks (not always up-to-date)
    pub relallvisible: i32,
    /// Number of all-frozen blocks (not always up-to-date)
    pub relallfrozen: i32,
    /// OID of TOAST table for this relation; 0 if none
    pub reltoastrelid: u32,
    /// True if the relation has (or has had) indexes
    pub relhasindex: bool,
    /// True if the relation is shared across all databases
    pub relisshared: bool,
    /// Persistence: 'p' = permanent, 'u' = unlogged, 't' = temporary
    pub relpersistence: u8,
    /// Relation kind: 'r' = table, 'i' = index, 'S' = sequence,
    /// 't' = TOAST, 'v' = view, 'm' = materialized view,
    /// 'c' = composite type, 'f' = foreign table, 'p' = partitioned table,
    /// 'I' = partitioned index
    pub relkind: u8,
    /// Number of user-visible columns (system columns excluded)
    pub relnatts: i16,
    /// Number of CHECK constraints
    pub relchecks: i16,
    /// True if the relation has (or has had) rules
    pub relhasrules: bool,
    /// True if the relation has (or has had) triggers
    pub relhastriggers: bool,
    /// True if the relation has (or has had) child tables or indexes
    pub relhassubclass: bool,
    /// True if row-level security is enabled
    pub relrowsecurity: bool,
    /// True if row-level security is forced for the table owner
    pub relforcerowsecurity: bool,
    /// True if the materialized view currently holds query results
    pub relispopulated: bool,
    /// Replica identity setting: 'd' = default (pkey), 'n' = nothing,
    /// 'f' = all columns, 'i' = index
    pub relreplident: u8,
    /// True if this relation is a partition
    pub relispartition: bool,
    /// During table rewrite, OID of the original relation; otherwise 0
    pub relrewrite: u32,
    /// All transaction IDs before this value are frozen in this relation
    pub relfrozenxid: u32,
    /// All MultiXactIds in this relation are >= this value
    pub relminmxid: u32,
}

use super::PgCatalogRelation;

use super::codec::PgDatum;
use crate::file::error::{PgError, Result};
use crate::heap::tuple::{ColumnSearchArg, HeapTupleData};
use crate::types::PgSchema;

// ────────────────────────────────────────────────────────────────────────────
// Datum → Rust type extraction helpers (shared by all from_row impls)
// ────────────────────────────────────────────────────────────────────────────

/// Decode the column at `col_index`, advance `*offset` by the datum size.
/// Returns `None` for NULL columns (offset unchanged).
///
/// Applies PostgreSQL's `att_align_pointer` rules before reading:
/// - For fixed-width types, always align to the type's nominal alignment.
/// - For varlena types, peek at the first byte: if non-zero (short varlena
///   or 1-byte external header), skip alignment; if zero (pad byte), align.
fn next_datum(
    tuple: &HeapTupleData,
    schema: &PgSchema,
    offset: &mut usize,
    col_index: usize,
) -> Result<Option<PgDatum>> {
    // NULL columns consume no space — no alignment, no data.
    if tuple.is_null(col_index) {
        return Ok(None);
    }

    // Apply alignment before reading the datum.
    let col = schema.column(col_index).unwrap();
    let align = col.type_id.align();
    if align > 1 {
        let is_varlena = matches!(col.type_id.type_len(), super::codec::PgTypeLen::Varlena);
        if is_varlena {
            // att_align_pointer: only align if the byte at offset is a pad byte (0x00).
            if tuple.data.get(*offset).is_none_or(|&b| b == 0) {
                *offset = (*offset + align - 1) & !(align - 1);
            }
        } else {
            // Fixed-width: always align.
            *offset = (*offset + align - 1) & !(align - 1);
        }
    }

    match tuple.get_column(ColumnSearchArg::ColumnIndex(col_index), schema, *offset) {
        Ok((datum, size)) => {
            *offset += size;
            Ok(Some(datum))
        }
        Err(PgError::NullColumnValue { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

fn as_u32(d: Option<PgDatum>) -> u32 {
    match d {
        Some(PgDatum::Oid(v)) | Some(PgDatum::Xid(v)) => v,
        _ => 0,
    }
}

fn as_i32(d: Option<PgDatum>) -> i32 {
    match d {
        Some(PgDatum::Int4(v)) => v,
        _ => 0,
    }
}

fn as_i16(d: Option<PgDatum>) -> i16 {
    match d {
        Some(PgDatum::Int2(v)) => v,
        _ => 0,
    }
}

fn as_f32(d: Option<PgDatum>) -> f32 {
    match d {
        Some(PgDatum::Float4(v)) => v,
        _ => 0.0,
    }
}

fn as_bool(d: Option<PgDatum>) -> bool {
    match d {
        Some(PgDatum::Bool(v)) => v,
        _ => false,
    }
}

fn as_u8(d: Option<PgDatum>) -> u8 {
    match d {
        Some(PgDatum::Char(v)) => v,
        _ => 0,
    }
}

#[allow(dead_code)]
fn as_name(d: Option<PgDatum>) -> [u8; 64] {
    match d {
        Some(PgDatum::Name(v)) => v,
        _ => [0u8; 64],
    }
}

fn as_name_str(d: Option<PgDatum>) -> String {
    match d {
        Some(PgDatum::Text(v)) => {
            // use std::ffi::CStr;
            // let column = CStr::from_bytes_until_nul(&v).unwrap().to_str();
            // String::from(column.unwrap())
            v
        }
        _ => String::new(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PgClass::from_row
// ────────────────────────────────────────────────────────────────────────────

impl PgClass {
    /// Build a `PgClass` from a decoded heap tuple row.
    ///
    /// Walks every fixed-width column in tuple order, decoding each datum
    /// and mapping it into the corresponding struct field. NULL columns
    /// receive zero/false defaults (most pg_class columns are NOT NULL,
    /// but the null bitmap must still be respected for correctness).
    pub fn from_row(tuple: &HeapTupleData, schema: &PgSchema) -> Result<Self> {
        let mut offset = 0usize;
        let mut col = |i| next_datum(tuple, schema, &mut offset, i);
        Ok(PgClass {
            oid: as_u32(col(0)?),
            relname: as_name_str(col(1)?),
            relnamespace: as_u32(col(2)?),
            reltype: as_u32(col(3)?),
            reloftype: as_u32(col(4)?),
            relowner: as_u32(col(5)?),
            relam: as_u32(col(6)?),
            relfilenode: as_u32(col(7)?),
            reltablespace: as_u32(col(8)?),
            relpages: as_i32(col(9)?),
            reltuples: as_f32(col(10)?),
            relallvisible: as_i32(col(11)?),
            relallfrozen: as_i32(col(12)?),
            reltoastrelid: as_u32(col(13)?),
            relhasindex: as_bool(col(14)?),
            relisshared: as_bool(col(15)?),
            relpersistence: as_u8(col(16)?),
            relkind: as_u8(col(17)?),
            relnatts: as_i16(col(18)?),
            relchecks: as_i16(col(19)?),
            relhasrules: as_bool(col(20)?),
            relhastriggers: as_bool(col(21)?),
            relhassubclass: as_bool(col(22)?),
            relrowsecurity: as_bool(col(23)?),
            relforcerowsecurity: as_bool(col(24)?),
            relispopulated: as_bool(col(25)?),
            relreplident: as_u8(col(26)?),
            relispartition: as_bool(col(27)?),
            relrewrite: as_u32(col(28)?),
            relfrozenxid: as_u32(col(29)?),
            relminmxid: as_u32(col(30)?),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PgType::from_row
// ────────────────────────────────────────────────────────────────────────────

impl PgType {
    /// Build a `PgType` from a decoded heap tuple row.
    pub fn from_row(tuple: &HeapTupleData, schema: &PgSchema) -> Result<Self> {
        let mut offset = 0usize;
        let mut col = |i| next_datum(tuple, schema, &mut offset, i);

        Ok(PgType {
            oid: as_u32(col(0)?),
            typname: as_name_str(col(1)?),
            typnamespace: as_u32(col(2)?),
            typowner: as_u32(col(3)?),
            typlen: as_i16(col(4)?),
            typbyval: as_bool(col(5)?),
            typtype: as_u8(col(6)?),
            typcategory: as_u8(col(7)?),
            typispreferred: as_bool(col(8)?),
            typisdefined: as_bool(col(9)?),
            typdelim: as_u8(col(10)?),
            typrelid: as_u32(col(11)?),
            typsubscript: as_u32(col(12)?),
            typelem: as_u32(col(13)?),
            typarray: as_u32(col(14)?),
            typinput: as_u32(col(15)?),
            typoutput: as_u32(col(16)?),
            typreceive: as_u32(col(17)?),
            typsend: as_u32(col(18)?),
            typmodin: as_u32(col(19)?),
            typmodout: as_u32(col(20)?),
            typanalyze: as_u32(col(21)?),
            typalign: as_u8(col(22)?),
            typstorage: as_u8(col(23)?),
            typnotnull: as_bool(col(24)?),
            typbasetype: as_u32(col(25)?),
            typtypmod: as_i32(col(26)?),
            typndims: as_i32(col(27)?),
            typcollation: as_u32(col(28)?),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PgAttribute::from_row
// ────────────────────────────────────────────────────────────────────────────

impl PgAttribute {
    /// Build a `PgAttribute` from a decoded heap tuple row.
    pub fn from_row(tuple: &HeapTupleData, schema: &PgSchema) -> Result<Self> {
        let mut offset = 0usize;
        let mut col = |i| next_datum(tuple, schema, &mut offset, i);

        Ok(PgAttribute {
            attrelid: as_u32(col(0)?),
            attname: as_name_str(col(1)?),
            atttypid: as_u32(col(2)?),
            attlen: as_i16(col(3)?),
            attnum: as_i16(col(4)?),
            atttypmod: as_i32(col(5)?),
            attndims: as_i16(col(6)?),
            attbyval: as_bool(col(7)?),
            attalign: as_u8(col(8)?),
            attstorage: as_u8(col(9)?),
            attcompression: as_u8(col(10)?),
            attnotnull: as_bool(col(11)?),
            atthasdef: as_bool(col(12)?),
            atthasmissing: as_bool(col(13)?),
            attidentity: as_u8(col(14)?),
            attgenerated: as_u8(col(15)?),
            attisdropped: as_bool(col(16)?),
            attislocal: as_bool(col(17)?),
            attinhcount: as_i16(col(18)?),
            attcollation: as_u32(col(19)?),
        })
    }
}

/// Column index constants for pg_class attributes.
///
/// These match the 1-based `attnum` values from `pg_attribute` for pg_class.
/// Useful when iterating heap tuples of `pg_class` and picking specific columns.
impl PgClass {
    // Attribute numbers (1-based, matching pg_attribute.attnum)
    pub const ANUM_OID: usize = 0;
    pub const ANUM_RELNAME: usize = 1;
    pub const ANUM_RELNAMESPACE: usize = 2;
    pub const ANUM_RELTYPE: usize = 3;
    pub const ANUM_RELOFTYPE: usize = 4;
    pub const ANUM_RELOWNER: usize = 5;
    pub const ANUM_RELAM: usize = 6;
    pub const ANUM_RELFILENODE: usize = 7;
    pub const ANUM_RELTABLESPACE: usize = 8;
    pub const ANUM_RELPAGES: usize = 9;
    pub const ANUM_RELTUPLES: usize = 10;
    pub const ANUM_RELALLVISIBLE: usize = 11;
    pub const ANUM_RELALLFROZEN: usize = 12;
    pub const ANUM_RELTOASTRELID: usize = 13;
    pub const ANUM_RELHASINDEX: usize = 14;
    pub const ANUM_RELISSHARED: usize = 15;
    pub const ANUM_RELPERSISTENCE: usize = 16;
    pub const ANUM_RELKIND: usize = 17;
    pub const ANUM_RELNATTS: usize = 18;
    pub const ANUM_RELCHECKS: usize = 19;
    pub const ANUM_RELHASRULES: usize = 20;
    pub const ANUM_RELHASTRIGGERS: usize = 21;
    pub const ANUM_RELHASSUBCLASS: usize = 22;
    pub const ANUM_RELROWSECURITY: usize = 23;
    pub const ANUM_RELFORCEROWSECURITY: usize = 24;
    pub const ANUM_RELISPOPULATED: usize = 25;
    pub const ANUM_RELREPLIDENT: usize = 26;
    pub const ANUM_RELISPARTITION: usize = 27;
    pub const ANUM_RELREWRITE: usize = 28;
    pub const ANUM_RELFROZENXID: usize = 29;
    pub const ANUM_RELMINMXID: usize = 30;

    /// The pg_class relation OID.
    pub const RELATION_OID: u32 = 1259;

    /// Number of fixed-width columns (excludes varlena trailing columns).
    pub const NUM_FIXED_ATTRS: usize = 31;

    /// The column type OIDs in tuple order, for driving the decoder.
    ///
    /// This allows generic tuple-walking code to decode pg_class rows
    /// without hardcoding types at every call site.
    pub const ATTR_TYPES: [super::codec::PgTypeId; Self::NUM_FIXED_ATTRS] = {
        use super::codec::PgTypeId as T;
        [
            T::Oid,    // oid
            T::Name,   // relname
            T::Oid,    // relnamespace
            T::Oid,    // reltype
            T::Oid,    // reloftype
            T::Oid,    // relowner
            T::Oid,    // relam
            T::Oid,    // relfilenode
            T::Oid,    // reltablespace
            T::Int4,   // relpages
            T::Float4, // reltuples
            T::Int4,   // relallvisible
            T::Int4,   // relallfrozen
            T::Oid,    // reltoastrelid
            T::Bool,   // relhasindex
            T::Bool,   // relisshared
            T::Char,   // relpersistence
            T::Char,   // relkind
            T::Int2,   // relnatts
            T::Int2,   // relchecks
            T::Bool,   // relhasrules
            T::Bool,   // relhastriggers
            T::Bool,   // relhassubclass
            T::Bool,   // relrowsecurity
            T::Bool,   // relforcerowsecurity
            T::Bool,   // relispopulated
            T::Char,   // relreplident
            T::Bool,   // relispartition
            T::Oid,    // relrewrite
            T::Xid,    // relfrozenxid
            T::Xid,    // relminmxid
        ]
    };

    pub const ATTR_NAMES: [&'static str; Self::NUM_FIXED_ATTRS] = [
        "oid",
        "relname",
        "relnamespace",
        "reltype",
        "reloftype",
        "relowner",
        "relam",
        "relfilenode",
        "reltablespace",
        "relpages",
        "reltuples",
        "relallvisible",
        "relallfrozen",
        "reltoastrelid",
        "relhasindex",
        "relisshared",
        "relpersistence",
        "relkind",
        "relnatts",
        "relchecks",
        "relhasrules",
        "relhastriggers",
        "relhassubclass",
        "relrowsecurity",
        "relforcerowsecurity",
        "relispopulated",
        "relreplident",
        "relispartition",
        "relrewrite",
        "relfrozenxid",
        "relminmxid",
    ];
}

impl PgCatalogRelation for PgClass {
    const RELATION_OID: u32 = PgClass::RELATION_OID;
    const NUM_FIXED_ATTRS: usize = PgClass::NUM_FIXED_ATTRS;

    fn attr_types() -> &'static [super::codec::PgTypeId] {
        &PgClass::ATTR_TYPES
    }
    fn attr_names() -> &'static [&'static str] {
        &PgClass::ATTR_NAMES
    }
    fn catalog_name() -> &'static str {
        "pg_class"
    }
}

// ────────────────────────────────────────────────────────────────────────────
// pg_type (OID 1247)
// ────────────────────────────────────────────────────────────────────────────

/// Rust representation of PostgreSQL's `pg_type` system catalog (OID 1247).
///
/// Mirrors the fixed-length portion of `CATALOG(pg_type)` from
/// `src/include/catalog/pg_type.h`. Variable-length trailing fields
/// (`typdefaultbin`, `typdefault`, `typacl`) are excluded.
#[derive(Debug, Clone)]
pub struct PgType {
    /// pg_type.oid
    pub oid: u32,
    /// Type name
    pub typname: String,
    /// OID of nam
    pub typnamespace: u32,
    /// Type owner
    pub typowner: u32,
    /// For fixed-size types, the number of bytes; negative for varlena (-1) or cstring (-2)
    pub typlen: i16,
    /// True if pass-by-value, false if pass-by-reference
    pub typbyval: bool,
    /// 'b' = base, 'c' = composite, 'd' = domain, 'e' = enum, 'p' = pseudo, 'r' = range, 'm' = multirange
    pub typtype: u8,
    /// Arbitrary classification used by the parser to choose preferred casts
    pub typcategory: u8,
    /// True if this type is preferred for implicit casts within its category
    pub typispreferred: bool,
    /// True if the type is fully defined (not a shell/placeholder)
    pub typisdefined: bool,
    /// Delimiter character for arrays of this type
    pub typdelim: u8,
    /// For composite types, the pg_class OID; 0 otherwise
    pub typrelid: u32,
    /// Subscript handler function OID (regproc)
    pub typsubscript: u32,
    /// If an array element type, OID of that element type; 0 otherwise
    pub typelem: u32,
    /// OID of the "true" array type that has this type as its element
    pub typarray: u32,
    /// Input function OID (text representation → internal)
    pub typinput: u32,
    /// Output function OID (internal → text representation)
    pub typoutput: u32,
    /// Binary receive function OID
    pub typreceive: u32,
    /// Binary send function OID
    pub typsend: u32,
    /// Type modifier input function OID (0 if none)
    pub typmodin: u32,
    /// Type modifier output function OID (0 if none)
    pub typmodout: u32,
    /// Custom ANALYZE function OID (0 if none)
    pub typanalyze: u32,
    /// Alignment: 'c' = char, 's' = short, 'i' = int, 'd' = double
    pub typalign: u8,
    /// TOAST storage strategy: 'p' = plain, 'x' = extended, 'm' = main, 'e' = external
    pub typstorage: u8,
    /// True if NOT NULL constraint exists
    pub typnotnull: bool,
    /// For domains, the base type OID; 0 otherwise
    pub typbasetype: u32,
    /// For domains, the typmod to apply to the base type; -1 otherwise
    pub typtypmod: i32,
    /// For array domains, the declared number of dimensions; 0 otherwise
    pub typndims: i32,
    /// Collation OID (0 if type does not support collation)
    pub typcollation: u32,
}

impl PgType {
    pub const RELATION_OID: u32 = 1247;
    pub const NUM_FIXED_ATTRS: usize = 29;

    pub const ATTR_TYPES: [super::codec::PgTypeId; Self::NUM_FIXED_ATTRS] = {
        use super::codec::PgTypeId as T;
        [
            T::Oid,  // oid
            T::Name, // typname
            T::Oid,  // typnamespace
            T::Oid,  // typowner
            T::Int2, // typlen
            T::Bool, // typbyval
            T::Char, // typtype
            T::Char, // typcategory
            T::Bool, // typispreferred
            T::Bool, // typisdefined
            T::Char, // typdelim
            T::Oid,  // typrelid
            T::Oid,  // typsubscript (regproc stored as Oid)
            T::Oid,  // typelem
            T::Oid,  // typarray
            T::Oid,  // typinput (regproc)
            T::Oid,  // typoutput (regproc)
            T::Oid,  // typreceive (regproc)
            T::Oid,  // typsend (regproc)
            T::Oid,  // typmodin (regproc)
            T::Oid,  // typmodout (regproc)
            T::Oid,  // typanalyze (regproc)
            T::Char, // typalign
            T::Char, // typstorage
            T::Bool, // typnotnull
            T::Oid,  // typbasetype
            T::Int4, // typtypmod
            T::Int4, // typndims
            T::Oid,  // typcollation
        ]
    };

    pub const ATTR_NAMES: [&'static str; Self::NUM_FIXED_ATTRS] = [
        "oid",
        "typname",
        "typnamespace",
        "typowner",
        "typlen",
        "typbyval",
        "typtype",
        "typcategory",
        "typispreferred",
        "typisdefined",
        "typdelim",
        "typrelid",
        "typsubscript",
        "typelem",
        "typarray",
        "typinput",
        "typoutput",
        "typreceive",
        "typsend",
        "typmodin",
        "typmodout",
        "typanalyze",
        "typalign",
        "typstorage",
        "typnotnull",
        "typbasetype",
        "typtypmod",
        "typndims",
        "typcollation",
    ];
}

impl PgCatalogRelation for PgType {
    const RELATION_OID: u32 = PgType::RELATION_OID;
    const NUM_FIXED_ATTRS: usize = PgType::NUM_FIXED_ATTRS;

    fn attr_types() -> &'static [super::codec::PgTypeId] {
        &PgType::ATTR_TYPES
    }
    fn attr_names() -> &'static [&'static str] {
        &PgType::ATTR_NAMES
    }
    fn catalog_name() -> &'static str {
        "pg_type"
    }
}

// ────────────────────────────────────────────────────────────────────────────
// pg_attribute (OID 1249)
// ────────────────────────────────────────────────────────────────────────────

/// Rust representation of PostgreSQL's `pg_attribute` system catalog (OID 1249).
///
/// Mirrors the fixed-length portion of `CATALOG(pg_attribute)` from
/// `src/include/catalog/pg_attribute.h`. Variable-length trailing fields
/// (`attstattarget`, `attacl`, `attoptions`, `attfdwoptions`, `attmissingval`)
/// are excluded.
///
/// **This is the most important catalog for pg_arrow** — it tells us each
/// column's type OID (`atttypid`), on-disk size (`attlen`), alignment
/// (`attalign`), and whether it has been dropped (`attisdropped`).
#[derive(Debug, Clone)]
pub struct PgAttribute {
    /// OID of the relation this column belongs to
    pub attrelid: u32,
    /// Column name
    pub attname: String,
    /// Type OID (references pg_type.oid) — the key field for decoding
    pub atttypid: u32,
    /// Copy of pg_type.typlen for this type
    pub attlen: i16,
    /// Attribute number (1-based for user columns, negative for system columns)
    pub attnum: i16,
    /// Type modifier (e.g. varchar(255) → typtypmod = 259)
    pub atttypmod: i32,
    /// Number of array dimensions (0 if not an array)
    pub attndims: i16,
    /// Copy of pg_type.typbyval
    pub attbyval: bool,
    /// Copy of pg_type.typalign: 'c', 's', 'i', or 'd'
    pub attalign: u8,
    /// Copy of pg_type.typstorage: 'p', 'x', 'm', or 'e'
    pub attstorage: u8,
    /// Compression method: 'p' = pglz, 'l' = lz4, '\0' = default
    pub attcompression: u8,
    /// True if column has NOT NULL constraint
    pub attnotnull: bool,
    /// True if column has a DEFAULT expression
    pub atthasdef: bool,
    /// True if column has a "missing" value for ADD COLUMN
    pub atthasmissing: bool,
    /// Identity column kind: 'a' = always, 'd' = by default, '\0' = not identity
    pub attidentity: u8,
    /// Generated column kind: 's' = stored, '\0' = not generated
    pub attgenerated: u8,
    /// True if column has been dropped (logically invisible, but still in tuple layout)
    pub attisdropped: bool,
    /// True if column has a local definition (not purely inherited)
    pub attislocal: bool,
    /// Number of direct parent relations this column is inherited from
    pub attinhcount: i16,
    /// Collation OID (0 if type does not use collation)
    pub attcollation: u32,
}

impl PgAttribute {
    pub const RELATION_OID: u32 = 1249;
    pub const NUM_FIXED_ATTRS: usize = 20;

    pub const ATTR_TYPES: [super::codec::PgTypeId; Self::NUM_FIXED_ATTRS] = {
        use super::codec::PgTypeId as T;
        [
            T::Oid,  // attrelid
            T::Name, // attname
            T::Oid,  // atttypid
            T::Int2, // attlen
            T::Int2, // attnum
            T::Int4, // atttypmod
            T::Int2, // attndims
            T::Bool, // attbyval
            T::Char, // attalign
            T::Char, // attstorage
            T::Char, // attcompression
            T::Bool, // attnotnull
            T::Bool, // atthasdef
            T::Bool, // atthasmissing
            T::Char, // attidentity
            T::Char, // attgenerated
            T::Bool, // attisdropped
            T::Bool, // attislocal
            T::Int2, // attinhcount
            T::Oid,  // attcollation
        ]
    };

    pub const ATTR_NAMES: [&'static str; Self::NUM_FIXED_ATTRS] = [
        "attrelid",
        "attname",
        "atttypid",
        "attlen",
        "attnum",
        "atttypmod",
        "attndims",
        "attbyval",
        "attalign",
        "attstorage",
        "attcompression",
        "attnotnull",
        "atthasdef",
        "atthasmissing",
        "attidentity",
        "attgenerated",
        "attisdropped",
        "attislocal",
        "attinhcount",
        "attcollation",
    ];
}

impl PgCatalogRelation for PgAttribute {
    const RELATION_OID: u32 = PgAttribute::RELATION_OID;
    const NUM_FIXED_ATTRS: usize = PgAttribute::NUM_FIXED_ATTRS;

    fn attr_types() -> &'static [super::codec::PgTypeId] {
        &PgAttribute::ATTR_TYPES
    }
    fn attr_names() -> &'static [&'static str] {
        &PgAttribute::ATTR_NAMES
    }
    fn catalog_name() -> &'static str {
        "pg_attribute"
    }
}

// ────────────────────────────────────────────────────────────────────────────
// pg_proc (OID 1255)
// ────────────────────────────────────────────────────────────────────────────

/// Rust representation of PostgreSQL's `pg_proc` system catalog (OID 1255).
///
/// Mirrors the fixed-length portion of `CATALOG(pg_proc)` from
/// `src/include/catalog/pg_proc.h`. Variable-length trailing fields
/// (`proargtypes`, `proallargtypes`, `proargmodes`, `proargnames`,
/// `proargdefaults`, `protrftypes`, `prosrc`, `probin`, `prosqlbody`,
/// `proconfig`, `proacl`) are excluded.
///
/// Note: `proargtypes` is `oidvector` which is actually varlena despite
/// logically being a fixed set of OIDs.
#[derive(Debug, Clone)]
pub struct PgProc {
    /// pg_proc.oid
    pub oid: u32,
    /// Procedure/function name
    pub proname: [u8; 64],
    /// OID of namespace containing this proc
    pub pronamespace: u32,
    /// Procedure owner
    pub proowner: u32,
    /// OID of the language this proc is written in
    pub prolang: u32,
    /// Estimated execution cost (in cpu_operator_cost units)
    pub procost: f32,
    /// Estimated number of result rows (0 for non-set-returning functions)
    pub prorows: f32,
    /// Element type of variadic array parameter; 0 if not variadic
    pub provariadic: u32,
    /// Planner support function OID (0 if none)
    pub prosupport: u32,
    /// 'f' = function, 'p' = procedure, 'a' = aggregate, 'w' = window
    pub prokind: u8,
    /// True if function is a security definer (runs as owner)
    pub prosecdef: bool,
    /// True if function has no side effects and reveals no info about args
    pub proleakproof: bool,
    /// True if function is strict (returns NULL on any NULL input)
    pub proisstrict: bool,
    /// True if function returns a set
    pub proretset: bool,
    /// Volatility: 'i' = immutable, 's' = stable, 'v' = volatile
    pub provolatile: u8,
    /// Parallel safety: 's' = safe, 'r' = restricted, 'u' = unsafe
    pub proparallel: u8,
    /// Number of input arguments (excludes OUT params)
    pub pronargs: i16,
    /// Number of arguments that have default values
    pub pronargdefaults: i16,
    /// OID of the return type
    pub prorettype: u32,
}

impl PgProc {
    pub const RELATION_OID: u32 = 1255;
    pub const NUM_FIXED_ATTRS: usize = 20;

    pub const ATTR_TYPES: [super::codec::PgTypeId; Self::NUM_FIXED_ATTRS] = {
        use super::codec::PgTypeId as T;
        [
            T::Oid,    // oid
            T::Name,   // proname
            T::Oid,    // pronamespace
            T::Oid,    // proowner
            T::Oid,    // prolang
            T::Float4, // procost
            T::Float4, // prorows
            T::Oid,    // provariadic
            T::Oid,    // prosupport (regproc)
            T::Char,   // prokind
            T::Bool,   // prosecdef
            T::Bool,   // proleakproof
            T::Bool,   // proisstrict
            T::Bool,   // proretset
            T::Char,   // provolatile
            T::Char,   // proparallel
            T::Int2,   // pronargs
            T::Int2,   // pronargdefaults
            T::Oid,    // prorettype
            T::Oid, // proargtypes (oidvector — varlena, but included as Oid for the first element)
        ]
    };

    pub const ATTR_NAMES: [&'static str; Self::NUM_FIXED_ATTRS] = [
        "oid",
        "proname",
        "pronamespace",
        "proowner",
        "prolang",
        "procost",
        "prorows",
        "provariadic",
        "prosupport",
        "prokind",
        "prosecdef",
        "proleakproof",
        "proisstrict",
        "proretset",
        "provolatile",
        "proparallel",
        "pronargs",
        "pronargdefaults",
        "prorettype",
        "proargtypes",
    ];
}

impl PgCatalogRelation for PgProc {
    const RELATION_OID: u32 = PgProc::RELATION_OID;
    const NUM_FIXED_ATTRS: usize = PgProc::NUM_FIXED_ATTRS;

    fn attr_types() -> &'static [super::codec::PgTypeId] {
        &PgProc::ATTR_TYPES
    }
    fn attr_names() -> &'static [&'static str] {
        &PgProc::ATTR_NAMES
    }
    fn catalog_name() -> &'static str {
        "pg_proc"
    }
}
