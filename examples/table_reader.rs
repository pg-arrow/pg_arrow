use pg_arrow::table::PgTableReader;
use pg_test_harness::db_oid_blocking;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let db_id = db_oid_blocking("postgres");

    // Bootstrap: reads pg_class + pg_attribute catalogs
    println!("Bootstrapping catalogs for db_id={db_id}...");
    let mut reader = PgTableReader::new(db_id)?;
    println!("Catalog bootstrap complete.");

    // Select a table
    reader.set_table("pgbench_accounts")?;
    println!("Schema: {:?}", reader.schema());

    // Fetch all rows
    let start = Instant::now();
    let rows = reader.fetch_by_limit(10_000_000)?;
    let duration = start.elapsed();
    println!("Elapsed: {:.3} ms", duration.as_secs_f64() * 1000.0);
    println!("Total rows: {}", rows.len());

    // Fetch all rows
    let start = Instant::now();
    let rows = reader.fetch_by_limit(10_000_000)?;
    let duration = start.elapsed();
    println!("Elapsed: {:.3} ms", duration.as_secs_f64() * 1000.0);
    println!("Total rows: {}", rows.len());

    // Fetch with limit
    let rows = reader.fetch_by_limit(5)?;
    for (_i, row) in rows.iter().enumerate() {
        println!("{}", row);
    }

    Ok(())
}
