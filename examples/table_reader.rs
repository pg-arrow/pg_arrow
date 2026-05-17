use pg_arrow::table::{PgTableReader, get_database_oid};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut args = std::env::args().skip(1);
    let data_dir = args.next().expect("usage: table_reader <pgdata> [db_name]");
    pg_arrow::file::set_data_dir(data_dir);
    let db_name = args.next().unwrap_or_else(|| "postgres".to_string());

    let db_id = get_database_oid(&db_name)?
        .ok_or_else(|| format!("database not found: {db_name}"))? as usize;

    // Bootstrap: reads pg_class + pg_attribute catalogs
    println!("Bootstrapping catalogs for db_id={db_id}...");
    let mut reader = PgTableReader::new(db_id)?;
    println!("Catalog bootstrap complete.");

    // Select a table
    reader.set_table("pgbench_accounts")?;
    println!("Schema: {:?}", reader.schema());

    // Fetch all rows
    let start = Instant::now();
    let rows = reader.fetch_all()?;
    let duration = start.elapsed();
    println!("Elapsed: {:.3} ms", duration.as_secs_f64() * 1000.0);
    println!("Total rows: {}", rows.len());

    // Print first 5
    for row in rows.iter().take(5) {
        println!("{}", row);
    }

    Ok(())
}
