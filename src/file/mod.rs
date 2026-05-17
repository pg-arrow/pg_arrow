pub mod error;
pub mod reader;
pub mod relation;

// Backward-compatible re-exports from the heap module.
// Consumers should migrate to `pg_arrow::heap::*` paths.
pub use crate::heap::page;
pub use crate::heap::tuple;
pub use crate::heap::{
    BlockIdData, ColumnSearchArg, HEAP_NATTS_MASK, HeapPageData, HeapTupleData,
    HeapTupleHeaderData, InfoMask, ItemIdData, ItemPointerData, LP_DEAD, LP_NORMAL, LP_REDIRECT,
    LP_UNUSED, PAGE_BUFFER_SIZE, PageHeaderData, PageXLogRecPtr, PgAttInfo,
    SIZEOF_HEAP_TUPLE_HEADER, align_to, read_line_pointer,
};
pub use crate::types::PgAlign;

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use toml::Table;

static DATA_DIR: OnceLock<String> = OnceLock::new();

/// Set the PostgreSQL data directory at runtime.
/// Must be called before any file reads. Can only be set once.
pub fn set_data_dir(path: String) {
    DATA_DIR.set(path).ok();
}

pub fn get_data_dir() -> Result<String, Box<dyn std::error::Error>> {
    // Prefer the runtime-configured path
    if let Some(dir) = DATA_DIR.get() {
        return Ok(dir.clone());
    }

    // Fall back to the TOML config (for tests)
    let config_path = if let Ok(p) = std::env::var("PG_ARROW_TEST_CONFIG") {
        PathBuf::from(p)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pg-test-config.toml")
    };
    let config_str = fs::read_to_string(config_path)?;

    let value = config_str.parse::<Table>()?;

    if let Some(data_dir) = value
        .get("postgres")
        .and_then(|v| v.get("pg18"))
        .and_then(|v| v.get("data_dir"))
        .and_then(|v| v.as_str())
    {
        Ok(data_dir.to_string())
    } else {
        Ok("".to_string())
    }
}

#[cfg(test)]
mod tests {

    use crate::file::error::PgError;
    use crate::types::PgAttribute;
    use crate::types::PgCatalogRelation;
    use crate::types::PgClass;
    use crate::util::pg_harness;

    use super::*;

    #[test]
    fn test_parse_filenode_map() {
        let data_dir = get_data_dir().unwrap();

        // Use global/pg_filenode.map: shared catalogs are always present.
        let buf = fs::read(format!("{}/global/pg_filenode.map", data_dir)).unwrap();
        let relmap = relation::parse_relmap(&buf).unwrap();

        assert!(relmap.num_mappings > 0);
        assert_eq!(relmap.mappings.len(), relmap.num_mappings as usize);

        // pg_database has stable catalog OID 1262 and is nailed-down (relmapped).
        println!("num_mappings = {}", relmap.num_mappings);
        println!("{:>6}  {:>10}", "oid", "filenode");
        for m in &relmap.mappings {
            println!("{:>6}  {:>10}", m.mapoid, m.mapfilenode);
        }
    }

    #[test]
    fn test_basic_page_header() {
        use reader::*;

        let table_reader = TableFileReader::new(16384, 16630);

        let mut page_reader = table_reader.get_page_reader().unwrap();
        let _pg_heap_page = page_reader.get_page_by_index(10).unwrap();
    }

    #[test]
    fn test_page_row_iter() {
        use env_logger::{Builder, Env};
        use reader::*;
        use std::fmt::Write;
        use std::hint::black_box;

        // Sets default to "debug" if RUST_LOG is not set
        let env = Env::default().default_filter_or("debug");
        Builder::from_env(env).init();

        let schema = PgClass::catalog_schema();

        let table_reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);

        let page_reader = table_reader.get_page_reader().unwrap();
        let mut row_str = String::new();
        for row in page_reader.into_iter().take(1_000_000).enumerate() {
            black_box(10);
            let mut offset = 0;
            let _ = writeln!(&mut row_str, "-----");
            for col_index in 0..schema.num_columns() {
                let tuple = row.1.as_ref().unwrap();
                match tuple.get_column(ColumnSearchArg::ColumnIndex(col_index), &schema, offset) {
                    Ok(column) => {
                        offset += column.1;
                        let _ = write!(&mut row_str, "{}  ", column.0);
                    }
                    Err(PgError::NullColumnValue { .. }) => {
                        // NULL columns consume 0 bytes of tuple data
                        let _ = write!(&mut row_str, "NULL  ");
                    }
                    Err(e) => panic!("unexpected decode error: {e}"),
                }
            }
            let _ = writeln!(&mut row_str);
        }
        println!("{}", row_str);
    }

    #[test]
    fn test_pgclass_parse() {
        use env_logger::{Builder, Env};
        use reader::*;
        use std::fmt::Write;
        use std::hint::black_box;

        // Sets default to "debug" if RUST_LOG is not set
        let env = Env::default().default_filter_or("debug");
        Builder::from_env(env).init();

        let schema = PgClass::catalog_schema();

        let table_reader = TableFileReader::new(16384, PgClass::RELATION_OID as usize);

        let page_reader = table_reader.get_page_reader().unwrap();
        let mut row_str = String::new();
        for row in page_reader.into_iter().take(1_000_000).enumerate() {
            black_box(10);
            let _ = writeln!(&mut row_str, "-----");
            let pg_class_row = PgClass::from_row(row.1.as_ref().unwrap(), &schema);
            let _ = writeln!(&mut row_str, "{:?}", pg_class_row.unwrap());
        }
        // println!("{}", row_str);
    }

    /// End-to-end catalog bootstrap test:
    ///   1. Scan pg_class to find a user table (relkind='r', oid > 16384)
    ///   2. Read pg_attribute rows for that table's OID
    ///   3. Build a PgSchema from those attributes
    ///   4. Read the actual table data using the dynamically-built schema
    #[test]
    fn test_catalog_bootstrap_read_user_table() {
        use reader::*;
        use std::fmt::Write;

        use crate::types::PgSchema;

        let db_id = pg_harness::db_oid_blocking("postgres");
        let target_table_name = "hits";

        // ── Step 1: Scan pg_class to find a user table ─────────────────────
        let pg_class_schema = PgClass::catalog_schema();
        let pg_class_reader = TableFileReader::new(db_id, PgClass::RELATION_OID as usize);

        let mut target_table: Option<PgClass> = None;
        for row_result in pg_class_reader.get_page_reader().unwrap().into_iter() {
            let tuple = row_result.unwrap();
            let pg_class_row = PgClass::from_row(&tuple, &pg_class_schema).unwrap();

            // Look for an ordinary table (relkind='r') in a user namespace (oid > 16384)
            // with a valid filenode (non-zero, meaning it has on-disk storage)
            if pg_class_row.relkind == b'r'
                // && pg_class_row.oid >= 16384
                && pg_class_row.relname == target_table_name
            {
                target_table = Some(pg_class_row);
                break;
            }
        }

        let table = target_table.expect("no user table found in pg_class");
        let table_name = table.relname;
        println!(
            "Found table: {} (oid={}, filenode={}, natts={})",
            table_name, table.oid, table.relfilenode, table.relnatts
        );

        // ── Step 2: Scan pg_attribute for this table's columns ─────────────
        let pg_attr_schema = PgAttribute::catalog_schema();
        let pg_attr_reader = TableFileReader::new(db_id, PgAttribute::RELATION_OID as usize);

        let mut table_attrs: Vec<PgAttribute> = Vec::new();
        for row_result in pg_attr_reader.get_page_reader().unwrap().into_iter() {
            let tuple = row_result.unwrap();
            let attr = PgAttribute::from_row(&tuple, &pg_attr_schema).unwrap();

            if attr.attrelid == table.oid {
                table_attrs.push(attr);
            }
        }

        assert!(
            !table_attrs.is_empty(),
            "no pg_attribute rows found for table {} (oid={})",
            table_name,
            table.oid
        );

        // ── Step 3: Build a PgSchema from the pg_attribute rows ────────────
        let user_schema = PgSchema::from_attributes(&table_name, &table_attrs);
        println!(
            "Schema for '{}': {} columns",
            table_name,
            user_schema.num_columns()
        );
        for (i, col) in user_schema.columns().enumerate() {
            println!(
                "  col[{}]: {} ({:?}, nullable={})",
                i, col.name, col.type_id, col.nullable
            );
        }

        assert!(
            user_schema.num_columns() > 0,
            "schema has no user-visible columns"
        );

        // ── Step 4: Read the actual table rows using the dynamic schema ────
        let table_reader = TableFileReader::new(db_id, table.relfilenode as usize);

        let mut row_count = 0usize;
        let mut output = String::new();
        for row_result in table_reader
            .get_page_reader()
            .unwrap()
            .into_iter()
            .take(100)
        {
            let tuple = row_result.unwrap();
            let mut offset = 0usize;
            let _ = write!(&mut output, "row[{row_count}]: ");
            for col_index in 0..user_schema.num_columns() {
                match tuple.get_column(
                    ColumnSearchArg::ColumnIndex(col_index),
                    &user_schema,
                    offset,
                ) {
                    Ok((datum, size)) => {
                        offset += size;
                        let _ = write!(&mut output, "{}  ", datum);
                    }
                    Err(PgError::NullColumnValue { .. }) => {
                        let _ = write!(&mut output, "NULL  ");
                    }
                    Err(e) => {
                        let _ = write!(&mut output, "ERR({e})  ");
                        break;
                    }
                }
            }
            let _ = writeln!(&mut output);
            row_count += 1;
        }

        println!("{output}");
        println!(
            "Read {row_count} rows from table '{table_name}' (oid={}, filenode={})",
            table.oid, table.relfilenode
        );
        assert!(row_count > 0, "expected at least one row in user table");
    }

    #[test]
    fn test_basic_tuple_header() {
        unimplemented!()
    }
}
