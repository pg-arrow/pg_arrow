pub mod arrow;
pub mod catalog;
pub mod codec;
pub mod column;
pub mod pg_type;
pub mod schema;

pub use catalog::PgCatalogRelation;
pub use column::PgColumn;
pub use schema::PgSchema;
pub use pg_type::{PgAttribute, PgClass, PgProc, PgType};
pub use codec::{
    DecodeError, PgAlign, PgDatum, PgTypeCategory, PgTypeId, PgTypeInfo, PgTypeLen, PgTypeStorage,
    decode_datum, decode_fixed, decode_varlena, read_varlena_header, skip_datum,
};
pub use arrow::{
    ColumnBuilder, decode_pg_numeric_i128, decode_pg_numeric_i256, extract_column_bytes,
    extract_fixed_bytes, numeric_typmod_to_arrow_type,
};

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
