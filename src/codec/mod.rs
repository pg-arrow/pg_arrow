use crate::heap::tuple::PgAlign;
use arrow::datatypes::{DataType, Field, IntervalUnit, TimeUnit};
use std::ffi::CStr;
use std::sync::Arc;

/// Helper: wrap a DataType in `List(Arc<Field>)` with a nullable "item" field.
fn list_of(dt: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", dt, true)))
}

// ────────────────────────────────────────────────────────────────────────────
// PostgreSQL type OID enum
// Source: pg_type catalog (SELECT * FROM pg_type WHERE typtype = 'b')
// ────────────────────────────────────────────────────────────────────────────

/// Known PostgreSQL base type OIDs.
///
/// Each variant's discriminant is the actual OID from `pg_type`.
/// Use `PgTypeId::try_from(oid)` to convert a raw `u32` from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PgTypeId {
    Bool = 16,
    Bytea = 17,
    Char = 18,
    Name = 19,
    Int8 = 20,
    Int2 = 21,
    Int4 = 23,
    Text = 25,
    Oid = 26,
    Tid = 27,
    Xid = 28,
    Cid = 29,
    Json = 114,
    Xml = 142,
    Point = 600,
    Lseg = 601,
    Path = 602,
    Box = 603,
    Polygon = 604,
    Line = 628,
    Cidr = 650,
    Float4 = 700,
    Float8 = 701,
    Circle = 718,
    Macaddr8 = 774,
    Money = 790,
    Macaddr = 829,
    Inet = 869,
    Aclitem = 1033,
    Bpchar = 1042,
    Varchar = 1043,
    Date = 1082,
    Time = 1083,
    Timestamp = 1114,
    Timestamptz = 1184,
    Interval = 1186,
    Timetz = 1266,
    Bit = 1560,
    Varbit = 1562,
    Numeric = 1700,
    Uuid = 2950,
    Jsonb = 3802,
    Jsonpath = 4072,
    Xid8 = 5069,

    // ── Array types (typarray values from pg_type) ───────────────────────
    BoolArray = 1000,
    ByteaArray = 1001,
    CharArray = 1002,
    NameArray = 1003,
    Int2Array = 1005,
    Int4Array = 1007,
    TextArray = 1009,
    TidArray = 1010,
    XidArray = 1011,
    CidArray = 1012,
    VarcharArray = 1015,
    Int8Array = 1016,
    Float4Array = 1021,
    Float8Array = 1022,
    OidArray = 1028,
    TimestampArray = 1115,
    DateArray = 1182,
    TimeArray = 1183,
    TimestamptzArray = 1185,
    NumericArray = 1231,
    UuidArray = 2951,
    JsonbArray = 3807,
}

impl TryFrom<u32> for PgTypeId {
    type Error = u32;

    /// Convert a raw OID from disk into a known type.
    /// Returns `Err(oid)` for unrecognized OIDs.
    fn try_from(oid: u32) -> Result<Self, u32> {
        match oid {
            16 => Ok(Self::Bool),
            17 => Ok(Self::Bytea),
            18 => Ok(Self::Char),
            19 => Ok(Self::Name),
            20 => Ok(Self::Int8),
            21 => Ok(Self::Int2),
            23 => Ok(Self::Int4),
            25 => Ok(Self::Text),
            26 => Ok(Self::Oid),
            27 => Ok(Self::Tid),
            28 => Ok(Self::Xid),
            29 => Ok(Self::Cid),
            114 => Ok(Self::Json),
            142 => Ok(Self::Xml),
            600 => Ok(Self::Point),
            601 => Ok(Self::Lseg),
            602 => Ok(Self::Path),
            603 => Ok(Self::Box),
            604 => Ok(Self::Polygon),
            628 => Ok(Self::Line),
            650 => Ok(Self::Cidr),
            700 => Ok(Self::Float4),
            701 => Ok(Self::Float8),
            718 => Ok(Self::Circle),
            774 => Ok(Self::Macaddr8),
            790 => Ok(Self::Money),
            829 => Ok(Self::Macaddr),
            869 => Ok(Self::Inet),
            1033 => Ok(Self::Aclitem),
            1042 => Ok(Self::Bpchar),
            1043 => Ok(Self::Varchar),
            1082 => Ok(Self::Date),
            1083 => Ok(Self::Time),
            1114 => Ok(Self::Timestamp),
            1184 => Ok(Self::Timestamptz),
            1186 => Ok(Self::Interval),
            1266 => Ok(Self::Timetz),
            1560 => Ok(Self::Bit),
            1562 => Ok(Self::Varbit),
            1700 => Ok(Self::Numeric),
            2950 => Ok(Self::Uuid),
            3802 => Ok(Self::Jsonb),
            4072 => Ok(Self::Jsonpath),
            5069 => Ok(Self::Xid8),
            // Array types
            1000 => Ok(Self::BoolArray),
            1001 => Ok(Self::ByteaArray),
            1002 => Ok(Self::CharArray),
            1003 => Ok(Self::NameArray),
            1005 => Ok(Self::Int2Array),
            1007 => Ok(Self::Int4Array),
            1009 => Ok(Self::TextArray),
            1010 => Ok(Self::TidArray),
            1011 => Ok(Self::XidArray),
            1012 => Ok(Self::CidArray),
            1015 => Ok(Self::VarcharArray),
            1016 => Ok(Self::Int8Array),
            1021 => Ok(Self::Float4Array),
            1022 => Ok(Self::Float8Array),
            1028 => Ok(Self::OidArray),
            1115 => Ok(Self::TimestampArray),
            1182 => Ok(Self::DateArray),
            1183 => Ok(Self::TimeArray),
            1185 => Ok(Self::TimestamptzArray),
            1231 => Ok(Self::NumericArray),
            2951 => Ok(Self::UuidArray),
            3807 => Ok(Self::JsonbArray),
            other => Err(other),
        }
    }
}

impl PgTypeId {
    /// The raw OID value as `u32`.
    pub const fn oid(self) -> u32 {
        self as u32
    }

    /// On-disk size classification. The tuple decoder uses this to know how
    /// many bytes to read (or whether to parse a varlena header).
    pub const fn type_len(self) -> PgTypeLen {
        match self {
            // 1-byte fixed
            Self::Bool | Self::Char => PgTypeLen::Fixed(1),
            // 2-byte fixed
            Self::Int2 => PgTypeLen::Fixed(2),
            // 4-byte fixed
            Self::Int4 | Self::Oid | Self::Xid | Self::Cid | Self::Float4 | Self::Date => {
                PgTypeLen::Fixed(4)
            }
            // 6-byte fixed
            Self::Tid | Self::Macaddr => PgTypeLen::Fixed(6),
            // 8-byte fixed
            Self::Int8
            | Self::Float8
            | Self::Money
            | Self::Time
            | Self::Timestamp
            | Self::Timestamptz
            | Self::Xid8
            | Self::Macaddr8 => PgTypeLen::Fixed(8),
            // 12-byte fixed
            Self::Timetz => PgTypeLen::Fixed(12),
            // 16-byte fixed
            Self::Interval | Self::Point | Self::Uuid | Self::Aclitem => PgTypeLen::Fixed(16),
            // 24-byte fixed
            Self::Line | Self::Circle => PgTypeLen::Fixed(24),
            // 32-byte fixed
            Self::Lseg | Self::Box => PgTypeLen::Fixed(32),
            // 64-byte fixed
            Self::Name => PgTypeLen::Fixed(64),
            // All varlena types (typlen = -1)
            Self::Text
            | Self::Varchar
            | Self::Bpchar
            | Self::Bytea
            | Self::Json
            | Self::Jsonb
            | Self::Jsonpath
            | Self::Xml
            | Self::Numeric
            | Self::Inet
            | Self::Cidr
            | Self::Bit
            | Self::Varbit
            | Self::Path
            | Self::Polygon => PgTypeLen::Varlena,
            // Array types are varlena
            Self::BoolArray
            | Self::ByteaArray
            | Self::CharArray
            | Self::NameArray
            | Self::Int2Array
            | Self::Int4Array
            | Self::TextArray
            | Self::TidArray
            | Self::XidArray
            | Self::CidArray
            | Self::VarcharArray
            | Self::Int8Array
            | Self::Float4Array
            | Self::Float8Array
            | Self::OidArray
            | Self::TimestampArray
            | Self::DateArray
            | Self::TimeArray
            | Self::TimestamptzArray
            | Self::NumericArray
            | Self::UuidArray
            | Self::JsonbArray => PgTypeLen::Varlena,
        }
    }

    /// Required alignment in bytes (1, 2, 4, or 8).
    /// The tuple decoder must align the read offset to this boundary before
    /// reading each column's data.
    pub const fn align(self) -> usize {
        match self {
            // Char-aligned (1 byte)
            Self::Bool | Self::Char | Self::Name | Self::Uuid => 1,
            // Short-aligned (2 bytes)
            Self::Int2 | Self::Tid => 2,
            // Int-aligned (4 bytes)
            Self::Int4
            | Self::Oid
            | Self::Xid
            | Self::Cid
            | Self::Float4
            | Self::Date
            | Self::Text
            | Self::Varchar
            | Self::Bpchar
            | Self::Bytea
            | Self::Json
            | Self::Xml
            | Self::Jsonb
            | Self::Jsonpath
            | Self::Numeric
            | Self::Inet
            | Self::Cidr
            | Self::Macaddr
            | Self::Macaddr8
            | Self::Bit
            | Self::Varbit => 4,
            // Double-aligned (8 bytes)
            Self::Int8
            | Self::Float8
            | Self::Money
            | Self::Xid8
            | Self::Time
            | Self::Timestamp
            | Self::Timestamptz
            | Self::Interval
            | Self::Timetz
            | Self::Point
            | Self::Line
            | Self::Lseg
            | Self::Box
            | Self::Circle
            | Self::Path
            | Self::Polygon
            | Self::Aclitem => 8,
            // Array types: int-aligned (varlena header is 4-byte)
            Self::BoolArray
            | Self::ByteaArray
            | Self::CharArray
            | Self::NameArray
            | Self::Int2Array
            | Self::Int4Array
            | Self::TextArray
            | Self::TidArray
            | Self::XidArray
            | Self::CidArray
            | Self::VarcharArray
            | Self::Int8Array
            | Self::Float4Array
            | Self::Float8Array
            | Self::OidArray
            | Self::TimestampArray
            | Self::DateArray
            | Self::TimeArray
            | Self::TimestamptzArray
            | Self::NumericArray
            | Self::UuidArray
            | Self::JsonbArray => 4,
        }
    }

    /// Maps this PostgreSQL type to its Arrow `DataType` representation.
    pub fn arrow_type(self) -> DataType {
        match self {
            Self::Bool => DataType::Boolean,
            Self::Int2 => DataType::Int16,
            Self::Int4 => DataType::Int32,
            Self::Int8 => DataType::Int64,
            Self::Float4 => DataType::Float32,
            Self::Float8 => DataType::Float64,
            Self::Oid => DataType::UInt32,
            Self::Xid => DataType::UInt32,
            Self::Cid => DataType::UInt32,
            Self::Xid8 => DataType::UInt64,
            Self::Date => DataType::Date32,
            Self::Time => DataType::Time64(TimeUnit::Microsecond),
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
            Self::Timestamptz => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            Self::Interval => DataType::Interval(IntervalUnit::MonthDayNano),
            Self::Timetz => DataType::Time64(TimeUnit::Microsecond),
            Self::Money => DataType::Int64,
            Self::Char => DataType::UInt8,
            Self::Name => DataType::Utf8,
            Self::Text => DataType::Utf8,
            Self::Varchar => DataType::Utf8,
            Self::Bpchar => DataType::Utf8,
            Self::Json => DataType::Utf8,
            Self::Xml => DataType::Utf8,
            Self::Numeric => DataType::Decimal256(38, 0),
            Self::Bytea => DataType::Binary,
            Self::Jsonb => DataType::Binary,
            Self::Jsonpath => DataType::Binary,
            Self::Uuid => DataType::FixedSizeBinary(16),
            Self::Macaddr => DataType::FixedSizeBinary(6),
            Self::Macaddr8 => DataType::FixedSizeBinary(8),
            Self::Inet => DataType::Binary,
            Self::Cidr => DataType::Binary,
            Self::Tid => DataType::Binary,
            Self::Aclitem => DataType::Binary,
            Self::Point => DataType::FixedSizeBinary(16),
            Self::Line => DataType::FixedSizeBinary(24),
            Self::Lseg => DataType::FixedSizeBinary(32),
            Self::Box => DataType::FixedSizeBinary(32),
            Self::Circle => DataType::FixedSizeBinary(24),
            Self::Path => DataType::Binary,
            Self::Polygon => DataType::Binary,
            Self::Bit => DataType::Binary,
            Self::Varbit => DataType::Binary,
            // Array types → Arrow List of element type
            Self::BoolArray => list_of(DataType::Boolean),
            Self::ByteaArray => list_of(DataType::Binary),
            Self::CharArray => list_of(DataType::UInt8),
            Self::NameArray => list_of(DataType::Utf8),
            Self::Int2Array => list_of(DataType::Int16),
            Self::Int4Array => list_of(DataType::Int32),
            Self::Int8Array => list_of(DataType::Int64),
            Self::Float4Array => list_of(DataType::Float32),
            Self::Float8Array => list_of(DataType::Float64),
            Self::TextArray => list_of(DataType::Utf8),
            Self::VarcharArray => list_of(DataType::Utf8),
            Self::OidArray => list_of(DataType::UInt32),
            Self::TidArray => list_of(DataType::Binary),
            Self::XidArray => list_of(DataType::UInt32),
            Self::CidArray => list_of(DataType::UInt32),
            Self::TimestampArray => list_of(DataType::Timestamp(TimeUnit::Microsecond, None)),
            Self::TimestamptzArray => list_of(DataType::Timestamp(
                TimeUnit::Microsecond,
                Some("UTC".into()),
            )),
            Self::DateArray => list_of(DataType::Date32),
            Self::TimeArray => list_of(DataType::Time64(TimeUnit::Microsecond)),
            Self::NumericArray => list_of(DataType::Decimal256(38, 0)),
            Self::UuidArray => list_of(DataType::FixedSizeBinary(16)),
            Self::JsonbArray => list_of(DataType::Binary),
        }
    }

    /// Returns the `PgTypeInfo` metadata bundle for this type.
    pub const fn info(self) -> PgTypeInfo {
        use PgTypeCategory as C;
        use PgTypeStorage as S;
        match self {
            Self::Bool => PgTypeInfo {
                oid: 16,
                name: "bool",
                typlen: 1,
                typbyval: true,
                category: C::Boolean,
                align: PgAlign::Char,
                storage: S::Plain,
            },
            Self::Bytea => PgTypeInfo {
                oid: 17,
                name: "bytea",
                typlen: -1,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Char => PgTypeInfo {
                oid: 18,
                name: "char",
                typlen: 1,
                typbyval: true,
                category: C::Internal,
                align: PgAlign::Char,
                storage: S::Plain,
            },
            Self::Name => PgTypeInfo {
                oid: 19,
                name: "name",
                typlen: 64,
                typbyval: false,
                category: C::String,
                align: PgAlign::Char,
                storage: S::Plain,
            },
            Self::Int8 => PgTypeInfo {
                oid: 20,
                name: "int8",
                typlen: 8,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Int2 => PgTypeInfo {
                oid: 21,
                name: "int2",
                typlen: 2,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Short,
                storage: S::Plain,
            },
            Self::Int4 => PgTypeInfo {
                oid: 23,
                name: "int4",
                typlen: 4,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Text => PgTypeInfo {
                oid: 25,
                name: "text",
                typlen: -1,
                typbyval: false,
                category: C::String,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Oid => PgTypeInfo {
                oid: 26,
                name: "oid",
                typlen: 4,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Tid => PgTypeInfo {
                oid: 27,
                name: "tid",
                typlen: 6,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Short,
                storage: S::Plain,
            },
            Self::Xid => PgTypeInfo {
                oid: 28,
                name: "xid",
                typlen: 4,
                typbyval: true,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Cid => PgTypeInfo {
                oid: 29,
                name: "cid",
                typlen: 4,
                typbyval: true,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Json => PgTypeInfo {
                oid: 114,
                name: "json",
                typlen: -1,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Xml => PgTypeInfo {
                oid: 142,
                name: "xml",
                typlen: -1,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Point => PgTypeInfo {
                oid: 600,
                name: "point",
                typlen: 16,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Lseg => PgTypeInfo {
                oid: 601,
                name: "lseg",
                typlen: 32,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Path => PgTypeInfo {
                oid: 602,
                name: "path",
                typlen: -1,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Extended,
            },
            Self::Box => PgTypeInfo {
                oid: 603,
                name: "box",
                typlen: 32,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Polygon => PgTypeInfo {
                oid: 604,
                name: "polygon",
                typlen: -1,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Extended,
            },
            Self::Line => PgTypeInfo {
                oid: 628,
                name: "line",
                typlen: 24,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Cidr => PgTypeInfo {
                oid: 650,
                name: "cidr",
                typlen: -1,
                typbyval: false,
                category: C::NetworkAddress,
                align: PgAlign::Int,
                storage: S::Main,
            },
            Self::Float4 => PgTypeInfo {
                oid: 700,
                name: "float4",
                typlen: 4,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Float8 => PgTypeInfo {
                oid: 701,
                name: "float8",
                typlen: 8,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Circle => PgTypeInfo {
                oid: 718,
                name: "circle",
                typlen: 24,
                typbyval: false,
                category: C::Geometric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Macaddr8 => PgTypeInfo {
                oid: 774,
                name: "macaddr8",
                typlen: 8,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Money => PgTypeInfo {
                oid: 790,
                name: "money",
                typlen: 8,
                typbyval: true,
                category: C::Numeric,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Macaddr => PgTypeInfo {
                oid: 829,
                name: "macaddr",
                typlen: 6,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Inet => PgTypeInfo {
                oid: 869,
                name: "inet",
                typlen: -1,
                typbyval: false,
                category: C::NetworkAddress,
                align: PgAlign::Int,
                storage: S::Main,
            },
            Self::Aclitem => PgTypeInfo {
                oid: 1033,
                name: "aclitem",
                typlen: 16,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Bpchar => PgTypeInfo {
                oid: 1042,
                name: "bpchar",
                typlen: -1,
                typbyval: false,
                category: C::String,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Varchar => PgTypeInfo {
                oid: 1043,
                name: "varchar",
                typlen: -1,
                typbyval: false,
                category: C::String,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Date => PgTypeInfo {
                oid: 1082,
                name: "date",
                typlen: 4,
                typbyval: true,
                category: C::DateTime,
                align: PgAlign::Int,
                storage: S::Plain,
            },
            Self::Time => PgTypeInfo {
                oid: 1083,
                name: "time",
                typlen: 8,
                typbyval: true,
                category: C::DateTime,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Timestamp => PgTypeInfo {
                oid: 1114,
                name: "timestamp",
                typlen: 8,
                typbyval: true,
                category: C::DateTime,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Timestamptz => PgTypeInfo {
                oid: 1184,
                name: "timestamptz",
                typlen: 8,
                typbyval: true,
                category: C::DateTime,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Interval => PgTypeInfo {
                oid: 1186,
                name: "interval",
                typlen: 16,
                typbyval: false,
                category: C::Timespan,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Timetz => PgTypeInfo {
                oid: 1266,
                name: "timetz",
                typlen: 12,
                typbyval: false,
                category: C::DateTime,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            Self::Bit => PgTypeInfo {
                oid: 1560,
                name: "bit",
                typlen: -1,
                typbyval: false,
                category: C::BitString,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Varbit => PgTypeInfo {
                oid: 1562,
                name: "varbit",
                typlen: -1,
                typbyval: false,
                category: C::BitString,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Numeric => PgTypeInfo {
                oid: 1700,
                name: "numeric",
                typlen: -1,
                typbyval: false,
                category: C::Numeric,
                align: PgAlign::Int,
                storage: S::Main,
            },
            Self::Uuid => PgTypeInfo {
                oid: 2950,
                name: "uuid",
                typlen: 16,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Char,
                storage: S::Plain,
            },
            Self::Jsonb => PgTypeInfo {
                oid: 3802,
                name: "jsonb",
                typlen: -1,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Jsonpath => PgTypeInfo {
                oid: 4072,
                name: "jsonpath",
                typlen: -1,
                typbyval: false,
                category: C::UserDefined,
                align: PgAlign::Int,
                storage: S::Extended,
            },
            Self::Xid8 => PgTypeInfo {
                oid: 5069,
                name: "xid8",
                typlen: 8,
                typbyval: true,
                category: C::UserDefined,
                align: PgAlign::Double,
                storage: S::Plain,
            },
            // Array types share common metadata (varlena, int-aligned, extended)
            _ => PgTypeInfo {
                oid: self as u32,
                name: "array",
                typlen: -1,
                typbyval: false,
                category: C::Array,
                align: PgAlign::Int,
                storage: S::Extended,
            },
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Type category (from pg_type.typcategory)
// ────────────────────────────────────────────────────────────────────────────

/// PostgreSQL type categories as defined in pg_type.typcategory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgTypeCategory {
    /// 'B' - Boolean types
    Boolean,
    /// 'N' - Numeric types
    Numeric,
    /// 'S' - String types
    String,
    /// 'D' - Date/time types
    DateTime,
    /// 'T' - Timespan types (interval)
    Timespan,
    /// 'G' - Geometric types
    Geometric,
    /// 'I' - Network address types
    NetworkAddress,
    /// 'V' - Bit string types
    BitString,
    /// 'U' - User-defined / misc types
    UserDefined,
    /// 'A' - Array types
    Array,
    /// 'Z' - Internal-use types
    Internal,
}

// ────────────────────────────────────────────────────────────────────────────
// TOAST storage strategy (from pg_type.typstorage)
// ────────────────────────────────────────────────────────────────────────────

/// TOAST storage strategy for a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgTypeStorage {
    /// 'p' - Plain: always stored inline, never compressed or out-of-line.
    Plain,
    /// 'x' - Extended: can be compressed and/or moved out-of-line.
    Extended,
    /// 'm' - Main: can be compressed but not moved out-of-line.
    Main,
    /// 'e' - External: can be moved out-of-line but not compressed.
    External,
}

// ────────────────────────────────────────────────────────────────────────────
// Type metadata bundle
// ────────────────────────────────────────────────────────────────────────────

/// Static metadata for a PostgreSQL base type, derived from pg_type.
#[derive(Debug, Clone, Copy)]
pub struct PgTypeInfo {
    /// pg_type.oid
    pub oid: u32,
    /// pg_type.typname
    pub name: &'static str,
    /// pg_type.typlen: -1 = varlena, -2 = cstring, >0 = fixed width in bytes
    pub typlen: i16,
    /// pg_type.typbyval: true if value fits in a Datum (pass-by-value)
    pub typbyval: bool,
    /// pg_type.typcategory
    pub category: PgTypeCategory,
    /// pg_type.typalign
    pub align: PgAlign,
    /// pg_type.typstorage
    pub storage: PgTypeStorage,
}

// ────────────────────────────────────────────────────────────────────────────
// On-disk size classification
// ────────────────────────────────────────────────────────────────────────────

/// On-disk size classification for a PostgreSQL type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgTypeLen {
    /// Fixed-width type: always exactly this many bytes on disk.
    Fixed(u16),
    /// Variable-length (varlena): has a 1-byte or 4-byte length header.
    /// The caller must read the varlena header to determine actual length.
    Varlena,
    /// Null-terminated C string (typlen = -2). Rare; only used by `cstring`.
    CString,
}

// ────────────────────────────────────────────────────────────────────────────
// Parsed datum — holds a decoded column value
// ────────────────────────────────────────────────────────────────────────────

/// A decoded PostgreSQL datum. Each variant holds the Rust-native representation
/// of the value after reading it from the on-disk tuple format.
#[derive(Debug, Clone, PartialEq)]
pub enum PgDatum {
    // ── Fixed-width, pass-by-value ───────────────────────────────────────
    Bool(bool),
    Int2(i16),
    Int4(i32),
    Int8(i64),
    Float4(f32),
    Float8(f64),
    Oid(u32),
    Date(i32),
    Time(i64),
    Timestamp(i64),
    TimestampTz(i64),
    Money(i64),
    Xid(u32),
    Cid(u32),
    Xid8(u64),

    // ── Fixed-width, pass-by-reference ───────────────────────────────────
    Char(u8),
    Name([u8; 64]),
    Tid {
        block: u32,
        offset: u16,
    },
    MacAddr([u8; 6]),
    MacAddr8([u8; 8]),
    Uuid([u8; 16]),
    Interval {
        microseconds: i64,
        days: i32,
        months: i32,
    },
    TimeTz {
        time_usec: i64,
        tz_offset: i32,
    },
    Point {
        x: f64,
        y: f64,
    },
    Line {
        a: f64,
        b: f64,
        c: f64,
    },
    Lseg {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Box {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Circle {
        x: f64,
        y: f64,
        radius: f64,
    },

    // ── Variable-length (varlena) ────────────────────────────────────────
    Text(String),
    Varchar(String),
    BpChar(String),
    Bytea(Vec<u8>),
    Json(String),
    Jsonb(Vec<u8>),
    JsonPath(Vec<u8>),
    Xml(String),
    Numeric(Vec<u8>),
    Inet(Vec<u8>),
    Cidr(Vec<u8>),
    Bit(Vec<u8>),
    VarBit(Vec<u8>),
    Path(Vec<u8>),
    Polygon(Vec<u8>),

    /// Fallback for types not yet decoded — stores raw bytes.
    RawBytes {
        oid: u32,
        data: Vec<u8>,
    },

    /// SQL NULL
    Null,
}

impl std::fmt::Display for PgDatum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Pass-by-value scalars
            PgDatum::Bool(v) => write!(f, "{v}"),
            PgDatum::Int2(v) => write!(f, "{v}"),
            PgDatum::Int4(v) => write!(f, "{v}"),
            PgDatum::Int8(v) => write!(f, "{v}"),
            PgDatum::Float4(v) => write!(f, "{v}"),
            PgDatum::Float8(v) => write!(f, "{v}"),
            PgDatum::Oid(v) => write!(f, "{v}"),
            PgDatum::Date(v) => write!(f, "{v}"),
            PgDatum::Time(v) => write!(f, "{v}"),
            PgDatum::Timestamp(v) => write!(f, "{v}"),
            PgDatum::TimestampTz(v) => write!(f, "{v}"),
            PgDatum::Money(v) => write!(f, "{v}"),
            PgDatum::Xid(v) => write!(f, "{v}"),
            PgDatum::Cid(v) => write!(f, "{v}"),
            PgDatum::Xid8(v) => write!(f, "{v}"),

            // Char / Name
            PgDatum::Char(v) => write!(f, "{}", *v as char),
            PgDatum::Name(bytes) => {
                use std::ffi::CStr;
                let column = CStr::from_bytes_until_nul(bytes).unwrap().to_str();
                write!(f, "{}", String::from(column.unwrap()))
            }

            // Composite fixed-width
            PgDatum::Tid { block, offset } => write!(f, "({block},{offset})"),
            PgDatum::MacAddr(b) => write!(
                f,
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                b[0], b[1], b[2], b[3], b[4], b[5]
            ),
            PgDatum::MacAddr8(b) => write!(
                f,
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]
            ),
            PgDatum::Uuid(b) => write!(
                f,
                "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                u16::from_be_bytes([b[4], b[5]]),
                u16::from_be_bytes([b[6], b[7]]),
                u16::from_be_bytes([b[8], b[9]]),
                // lower 6 bytes as a single u64 (padded)
                u64::from_be_bytes([0, 0, b[10], b[11], b[12], b[13], b[14], b[15]])
            ),
            PgDatum::Interval {
                microseconds,
                days,
                months,
            } => write!(f, "{months} mons {days} days {microseconds} usecs"),
            PgDatum::TimeTz {
                time_usec,
                tz_offset,
            } => write!(f, "{time_usec} usecs tz={tz_offset}"),
            PgDatum::Point { x, y } => write!(f, "({x},{y})"),
            PgDatum::Line { a, b, c } => write!(f, "{{{a},{b},{c}}}"),
            PgDatum::Lseg { x1, y1, x2, y2 } => write!(f, "[({x1},{y1}),({x2},{y2})]"),
            PgDatum::Box { x1, y1, x2, y2 } => write!(f, "({x1},{y1}),({x2},{y2})"),
            PgDatum::Circle { x, y, radius } => write!(f, "<({x},{y}),{radius}>"),

            // Variable-length strings
            PgDatum::Text(s)
            | PgDatum::Varchar(s)
            | PgDatum::BpChar(s)
            | PgDatum::Json(s)
            | PgDatum::Xml(s) => write!(f, "{s}"),

            // Variable-length binary — hex-encode
            PgDatum::Bytea(b)
            | PgDatum::Jsonb(b)
            | PgDatum::JsonPath(b)
            | PgDatum::Numeric(b)
            | PgDatum::Inet(b)
            | PgDatum::Cidr(b)
            | PgDatum::Bit(b)
            | PgDatum::VarBit(b)
            | PgDatum::Path(b)
            | PgDatum::Polygon(b) => {
                write!(f, "\\x")?;
                for byte in b {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }

            PgDatum::RawBytes { oid, data } => {
                write!(f, "(oid={oid})\\x")?;
                for byte in data {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }

            PgDatum::Null => write!(f, "NULL"),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PgDatum → Arrow DataType
// ────────────────────────────────────────────────────────────────────────────

impl PgDatum {
    /// Returns the Arrow `DataType` that this datum maps to.
    pub fn arrow_type(&self) -> DataType {
        match self {
            PgDatum::Bool(_) => DataType::Boolean,
            PgDatum::Int2(_) => DataType::Int16,
            PgDatum::Int4(_) => DataType::Int32,
            PgDatum::Int8(_) => DataType::Int64,
            PgDatum::Float4(_) => DataType::Float32,
            PgDatum::Float8(_) => DataType::Float64,
            PgDatum::Oid(_) => DataType::UInt32,
            PgDatum::Xid(_) => DataType::UInt32,
            PgDatum::Cid(_) => DataType::UInt32,
            PgDatum::Xid8(_) => DataType::UInt64,
            PgDatum::Date(_) => DataType::Date32,
            PgDatum::Time(_) => DataType::Time64(TimeUnit::Microsecond),
            PgDatum::Timestamp(_) => DataType::Timestamp(TimeUnit::Microsecond, None),
            PgDatum::TimestampTz(_) => {
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
            }
            PgDatum::Interval { .. } => DataType::Interval(IntervalUnit::MonthDayNano),
            PgDatum::TimeTz { .. } => DataType::Time64(TimeUnit::Microsecond),
            PgDatum::Money(_) => DataType::Int64,
            PgDatum::Char(_) => DataType::UInt8,
            PgDatum::Name(_) => DataType::Utf8,
            PgDatum::Tid { .. } => DataType::Binary,
            PgDatum::MacAddr(_) => DataType::FixedSizeBinary(6),
            PgDatum::MacAddr8(_) => DataType::FixedSizeBinary(8),
            PgDatum::Uuid(_) => DataType::FixedSizeBinary(16),
            PgDatum::Point { .. } => DataType::FixedSizeBinary(16),
            PgDatum::Line { .. } => DataType::FixedSizeBinary(24),
            PgDatum::Lseg { .. } => DataType::FixedSizeBinary(32),
            PgDatum::Box { .. } => DataType::FixedSizeBinary(32),
            PgDatum::Circle { .. } => DataType::FixedSizeBinary(24),
            PgDatum::Text(_) => DataType::Utf8,
            PgDatum::Varchar(_) => DataType::Utf8,
            PgDatum::BpChar(_) => DataType::Utf8,
            PgDatum::Json(_) => DataType::Utf8,
            PgDatum::Xml(_) => DataType::Utf8,
            PgDatum::Bytea(_) => DataType::Binary,
            PgDatum::Jsonb(_) => DataType::Binary,
            PgDatum::JsonPath(_) => DataType::Binary,
            PgDatum::Numeric(_) => DataType::Decimal256(38, 0),
            PgDatum::Inet(_) => DataType::Binary,
            PgDatum::Cidr(_) => DataType::Binary,
            PgDatum::Bit(_) => DataType::Binary,
            PgDatum::VarBit(_) => DataType::Binary,
            PgDatum::Path(_) => DataType::Binary,
            PgDatum::Polygon(_) => DataType::Binary,
            PgDatum::RawBytes { .. } => DataType::Binary,
            PgDatum::Null => DataType::Null,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Datum decoding — raw bytes → PgDatum
// ────────────────────────────────────────────────────────────────────────────

/// Error returned when bytes cannot be decoded into a `PgDatum`.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("buffer too short for {type_name}: need {expected} bytes, got {actual}")]
    BufferTooShort {
        type_name: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("invalid UTF-8 in {type_name} at offset {offset}: {source}")]
    InvalidUtf8 {
        type_name: &'static str,
        offset: usize,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("missing null terminator in name/cstring column")]
    MissingNulTerminator,
    #[error("invalid varlena header at offset {offset}: first byte = 0x{first_byte:02x}")]
    InvalidVarlenaHeader { offset: usize, first_byte: u8 },
}

/// Decode a fixed-width column from `bytes` according to `type_id`.
///
/// `bytes` must be exactly the right length for the type (caller slices
/// using `PgTypeId::type_len()`). PostgreSQL heap tuples store values in
/// the server's native byte order, which is little-endian on x86.
pub fn decode_fixed(type_id: PgTypeId, bytes: &[u8]) -> Result<PgDatum, DecodeError> {
    let len = bytes.len();
    match type_id {
        PgTypeId::Bool => {
            check_len("bool", 1, len)?;
            Ok(PgDatum::Bool(bytes[0] != 0))
        }
        PgTypeId::Char => {
            check_len("char", 1, len)?;
            Ok(PgDatum::Char(bytes[0]))
        }
        PgTypeId::Int2 => {
            check_len("int2", 2, len)?;
            Ok(PgDatum::Int2(i16::from_ne_bytes(
                bytes[..2].try_into().unwrap(),
            )))
        }
        PgTypeId::Int4 => {
            check_len("int4", 4, len)?;
            Ok(PgDatum::Int4(i32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Oid => {
            check_len("oid", 4, len)?;
            Ok(PgDatum::Oid(u32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Xid => {
            check_len("xid", 4, len)?;
            Ok(PgDatum::Xid(u32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Cid => {
            check_len("cid", 4, len)?;
            Ok(PgDatum::Cid(u32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Float4 => {
            check_len("float4", 4, len)?;
            Ok(PgDatum::Float4(f32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Date => {
            check_len("date", 4, len)?;
            Ok(PgDatum::Date(i32::from_ne_bytes(
                bytes[..4].try_into().unwrap(),
            )))
        }
        PgTypeId::Int8 => {
            check_len("int8", 8, len)?;
            Ok(PgDatum::Int8(i64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Float8 => {
            check_len("float8", 8, len)?;
            Ok(PgDatum::Float8(f64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Money => {
            check_len("money", 8, len)?;
            Ok(PgDatum::Money(i64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Time => {
            check_len("time", 8, len)?;
            Ok(PgDatum::Time(i64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Timestamp => {
            check_len("timestamp", 8, len)?;
            Ok(PgDatum::Timestamp(i64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Timestamptz => {
            check_len("timestamptz", 8, len)?;
            Ok(PgDatum::TimestampTz(i64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Xid8 => {
            check_len("xid8", 8, len)?;
            Ok(PgDatum::Xid8(u64::from_ne_bytes(
                bytes[..8].try_into().unwrap(),
            )))
        }
        PgTypeId::Macaddr8 => {
            check_len("macaddr8", 8, len)?;
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[..8]);
            Ok(PgDatum::MacAddr8(buf))
        }
        PgTypeId::Tid => {
            // ItemPointerData: 4 bytes BlockIdData + 2 bytes OffsetNumber
            check_len("tid", 6, len)?;
            let block = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
            let offset = u16::from_ne_bytes(bytes[4..6].try_into().unwrap());
            Ok(PgDatum::Tid { block, offset })
        }
        PgTypeId::Macaddr => {
            check_len("macaddr", 6, len)?;
            let mut buf = [0u8; 6];
            buf.copy_from_slice(&bytes[..6]);
            Ok(PgDatum::MacAddr(buf))
        }
        PgTypeId::Timetz => {
            // 8 bytes time (int64 microseconds) + 4 bytes tz offset (int32 seconds)
            check_len("timetz", 12, len)?;
            let time_usec = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let tz_offset = i32::from_ne_bytes(bytes[8..12].try_into().unwrap());
            Ok(PgDatum::TimeTz {
                time_usec,
                tz_offset,
            })
        }
        PgTypeId::Interval => {
            // 8 bytes microseconds + 4 bytes days + 4 bytes months
            check_len("interval", 16, len)?;
            let microseconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let days = i32::from_ne_bytes(bytes[8..12].try_into().unwrap());
            let months = i32::from_ne_bytes(bytes[12..16].try_into().unwrap());
            Ok(PgDatum::Interval {
                microseconds,
                days,
                months,
            })
        }
        PgTypeId::Point => {
            check_len("point", 16, len)?;
            let x = f64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let y = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
            Ok(PgDatum::Point { x, y })
        }
        PgTypeId::Uuid => {
            check_len("uuid", 16, len)?;
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&bytes[..16]);
            Ok(PgDatum::Uuid(buf))
        }
        PgTypeId::Aclitem => {
            check_len("aclitem", 16, len)?;
            Ok(PgDatum::RawBytes {
                oid: PgTypeId::Aclitem.oid(),
                data: bytes[..16].to_vec(),
            })
        }
        PgTypeId::Line => {
            // {A, B, C} coefficients, each float8
            check_len("line", 24, len)?;
            let a = f64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let b = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
            let c = f64::from_ne_bytes(bytes[16..24].try_into().unwrap());
            Ok(PgDatum::Line { a, b, c })
        }
        PgTypeId::Circle => {
            // center point (16 bytes) + radius (8 bytes)
            check_len("circle", 24, len)?;
            let x = f64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let y = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
            let radius = f64::from_ne_bytes(bytes[16..24].try_into().unwrap());
            Ok(PgDatum::Circle { x, y, radius })
        }
        PgTypeId::Lseg => {
            // two points: p1(x,y) + p2(x,y)
            check_len("lseg", 32, len)?;
            let x1 = f64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let y1 = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
            let x2 = f64::from_ne_bytes(bytes[16..24].try_into().unwrap());
            let y2 = f64::from_ne_bytes(bytes[24..32].try_into().unwrap());
            Ok(PgDatum::Lseg { x1, y1, x2, y2 })
        }
        PgTypeId::Box => {
            // high point + low point
            check_len("box", 32, len)?;
            let x1 = f64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let y1 = f64::from_ne_bytes(bytes[8..16].try_into().unwrap());
            let x2 = f64::from_ne_bytes(bytes[16..24].try_into().unwrap());
            let y2 = f64::from_ne_bytes(bytes[24..32].try_into().unwrap());
            Ok(PgDatum::Box { x1, y1, x2, y2 })
        }
        PgTypeId::Name => {
            // Fixed 64-byte null-padded C string
            check_len("name", 64, len)?;
            use std::ffi::CStr;
            let column = CStr::from_bytes_until_nul(&bytes[..64]).unwrap().to_str();

            Ok(PgDatum::Text(String::from(column.unwrap())))
        }
        // Varlena types should not reach here — caller must use decode_varlena
        _ => Ok(PgDatum::RawBytes {
            oid: type_id.oid(),
            data: bytes.to_vec(),
        }),
    }
}

/// Decode a varlena column from `bytes` (payload only, after varlena header
/// has been stripped by the caller).
///
/// `raw_payload` is the detoasted, decompressed payload bytes — the varlena
/// header and any TOAST indirection must already be resolved.
pub fn decode_varlena(type_id: PgTypeId, raw_payload: &[u8]) -> Result<PgDatum, DecodeError> {
    match type_id {
        // String types: validate UTF-8 and return as String
        PgTypeId::Text => {
            let s = std::str::from_utf8(raw_payload).map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "text",
                offset: e.valid_up_to(),
                source: e,
            })?;
            Ok(PgDatum::Text(s.to_owned()))
        }
        PgTypeId::Varchar => {
            let s = std::str::from_utf8(raw_payload).map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "varchar",
                offset: e.valid_up_to(),
                source: e,
            })?;
            Ok(PgDatum::Varchar(s.to_owned()))
        }
        PgTypeId::Bpchar => {
            let s = std::str::from_utf8(raw_payload).map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "bpchar",
                offset: e.valid_up_to(),
                source: e,
            })?;
            Ok(PgDatum::BpChar(s.to_owned()))
        }
        PgTypeId::Json => {
            let s = std::str::from_utf8(raw_payload).map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "json",
                offset: e.valid_up_to(),
                source: e,
            })?;
            Ok(PgDatum::Json(s.to_owned()))
        }
        PgTypeId::Xml => {
            let s = std::str::from_utf8(raw_payload).map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "xml",
                offset: e.valid_up_to(),
                source: e,
            })?;
            Ok(PgDatum::Xml(s.to_owned()))
        }
        // Binary-blob types: copy raw bytes
        PgTypeId::Bytea => Ok(PgDatum::Bytea(raw_payload.to_vec())),
        PgTypeId::Jsonb => Ok(PgDatum::Jsonb(raw_payload.to_vec())),
        PgTypeId::Jsonpath => Ok(PgDatum::JsonPath(raw_payload.to_vec())),
        PgTypeId::Numeric => Ok(PgDatum::Numeric(raw_payload.to_vec())),
        PgTypeId::Inet => Ok(PgDatum::Inet(raw_payload.to_vec())),
        PgTypeId::Cidr => Ok(PgDatum::Cidr(raw_payload.to_vec())),
        PgTypeId::Bit => Ok(PgDatum::Bit(raw_payload.to_vec())),
        PgTypeId::Varbit => Ok(PgDatum::VarBit(raw_payload.to_vec())),
        PgTypeId::Path => Ok(PgDatum::Path(raw_payload.to_vec())),
        PgTypeId::Polygon => Ok(PgDatum::Polygon(raw_payload.to_vec())),
        // Unknown or array types — store as raw bytes
        _ => Ok(PgDatum::RawBytes {
            oid: type_id.oid(),
            data: raw_payload.to_vec(),
        }),
    }
}

/// Read a varlena header from `data` at `offset`. Returns `(payload_start, payload_len)`.
///
/// PostgreSQL varlena encoding (little-endian, see `varatt.h`):
///
/// - **1-byte external** (`varattrib_1b_e`): first byte == `0x01` exactly.
///   This is a TOAST pointer. The second byte is the tag (`VARTAG_ONDISK` = 18),
///   followed by the `varatt_external` struct (16 bytes). Total on-disk size = 18
///   bytes for `VARTAG_ONDISK`. Since pg_arrow cannot yet detoast, we return an
///   empty payload so callers skip past the pointer cleanly.
///
/// - **1-byte header** (short varlena): first byte has bit 0 set (but != `0x01`).
///   The upper 7 bits encode the total length (header + data). Max 126 bytes of payload.
///
/// - **4-byte header** (regular varlena): the 4-byte word (native endian) has the
///   low 2 bits as tag. `00` = uncompressed, length in upper 30 bits.
///   `10` = compressed (PGLZ). The length includes the 4-byte header.
pub fn read_varlena_header(data: &[u8], offset: usize) -> Result<(usize, usize), DecodeError> {
    let first = *data.get(offset).ok_or(DecodeError::BufferTooShort {
        type_name: "varlena header",
        expected: 1,
        actual: 0,
    })?;

    if first == 0x01 {
        // External TOAST pointer (varattrib_1b_e): va_header=0x01, va_tag, va_data.
        // The tag byte determines the total size. On-disk heap tuples only use
        // VARTAG_ONDISK (18), which means sizeof(varatt_external)=16 + 2 header
        // bytes = 18 total. We read the tag to compute the correct size.
        let tag = *data.get(offset + 1).ok_or(DecodeError::BufferTooShort {
            type_name: "varlena external tag",
            expected: 2,
            actual: 1,
        })?;
        // VARTAG_ONDISK = 18: sizeof(varatt_external) = 16, total = 1+1+16 = 18.
        // The tag value was chosen to equal the total pointer size for on-disk.
        let pointer_size = match tag {
            18 => 18, // VARTAG_ONDISK
            _ => {
                return Err(DecodeError::InvalidVarlenaHeader {
                    offset,
                    first_byte: first,
                });
            }
        };
        // Return empty payload — we cannot detoast, so callers get an empty
        // slice for the value and the correct number of bytes consumed.
        Ok((offset + pointer_size, 0))
    } else if first & 0x01 != 0 {
        // Short varlena: 1-byte header, length in upper 7 bits
        let total_len = (first >> 1) as usize;
        let payload_start = offset + 1;
        let payload_len = total_len.saturating_sub(1);
        Ok((payload_start, payload_len))
    } else {
        // 4-byte header
        if data.len() < offset + 4 {
            return Err(DecodeError::BufferTooShort {
                type_name: "varlena header (4-byte)",
                expected: 4,
                actual: data.len() - offset,
            });
        }
        let header_word = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap());
        let tag = header_word & 0x03;
        match tag {
            0x00 => {
                // Uncompressed: length in upper 30 bits (includes 4-byte header)
                let total_len = (header_word >> 2) as usize;
                let payload_start = offset + 4;
                let payload_len = total_len.saturating_sub(4);
                Ok((payload_start, payload_len))
            }
            0x02 => {
                // Compressed (PGLZ): upper 30 bits = total length including header
                // TODO: decompress PGLZ payload before decoding
                let total_len = (header_word >> 2) as usize;
                let payload_start = offset + 4;
                let payload_len = total_len.saturating_sub(4);
                Ok((payload_start, payload_len))
            }
            _ => Err(DecodeError::InvalidVarlenaHeader {
                offset,
                first_byte: first,
            }),
        }
    }
}

/// Decode a single column from raw tuple data at `offset`, dispatching
/// to `decode_fixed` or `decode_varlena` based on the type's `type_len()`.
///
/// Returns `(datum, bytes_consumed)` so the caller can advance the offset.
pub fn decode_datum(
    type_id: PgTypeId,
    data: &[u8],
    offset: usize,
) -> Result<(PgDatum, usize), DecodeError> {
    match type_id.type_len() {
        PgTypeLen::Fixed(n) => {
            let n = n as usize;
            let end = offset + n;
            let bytes = data.get(offset..end).ok_or(DecodeError::BufferTooShort {
                type_name: type_id.info().name,
                expected: n,
                actual: data.len().saturating_sub(offset),
            })?;
            let datum = decode_fixed(type_id, bytes)?;
            Ok((datum, n))
        }
        PgTypeLen::Varlena => {
            let (payload_start, payload_len) = read_varlena_header(data, offset)?;
            let payload_end = payload_start + payload_len;
            let payload =
                data.get(payload_start..payload_end)
                    .ok_or(DecodeError::BufferTooShort {
                        type_name: type_id.info().name,
                        expected: payload_len,
                        actual: data.len().saturating_sub(payload_start),
                    })?;
            let datum = decode_varlena(type_id, payload)?;
            // Total bytes consumed = from offset to end of payload
            let consumed = payload_end - offset;
            Ok((datum, consumed))
        }
        PgTypeLen::CString => {
            // Scan for null terminator
            let cstr = CStr::from_bytes_until_nul(&data[offset..])
                .map_err(|_| DecodeError::MissingNulTerminator)?;
            let s = cstr.to_str().map_err(|e| DecodeError::InvalidUtf8 {
                type_name: "cstring",
                offset: e.valid_up_to(),
                source: e,
            })?;
            let consumed = s.len() + 1; // include the null terminator
            Ok((PgDatum::Text(s.to_owned()), consumed))
        }
    }
}

/// Compute the byte size of a datum at `offset` without decoding its value.
///
/// This is the lightweight counterpart to [`decode_datum`]: it walks the same
/// encoding but skips all parsing, allocation, and validation. Used by column
/// projection to advance past non-projected columns cheaply.
#[inline]
pub fn skip_datum(type_len: PgTypeLen, data: &[u8], offset: usize) -> Result<usize, DecodeError> {
    match type_len {
        PgTypeLen::Fixed(n) => {
            // For fixed-width types called from the hot loop, the caller
            // should use the `fixed_sizes` fast path instead. This branch
            // exists for the general-purpose API.
            Ok(n as usize)
        }
        PgTypeLen::Varlena => {
            let (payload_start, payload_len) = read_varlena_header(data, offset)?;
            Ok(payload_start + payload_len - offset)
        }
        PgTypeLen::CString => {
            // Scan for null terminator without UTF-8 validation or allocation.
            let remaining = data.get(offset..).ok_or(DecodeError::BufferTooShort {
                type_name: "cstring",
                expected: 1,
                actual: 0,
            })?;
            let nul_pos = remaining
                .iter()
                .position(|&b| b == 0)
                .ok_or(DecodeError::MissingNulTerminator)?;
            Ok(nul_pos + 1) // include the null terminator
        }
    }
}

#[inline]
fn check_len(type_name: &'static str, expected: usize, actual: usize) -> Result<(), DecodeError> {
    if actual < expected {
        Err(DecodeError::BufferTooShort {
            type_name,
            expected,
            actual,
        })
    } else {
        Ok(())
    }
}
