use std::path::PathBuf;

use std::fs;
use toml::Table;

const PAGE_BUFFER_SIZE: usize = 8 * 1024; // 8KB, configurable

fn get_data_dir() -> Result<String, Box<dyn std::error::Error>> {
    use std::env;

    let project_root_dir = env!("CARGO_MANIFEST_DIR");

    println!("Project root {}", project_root_dir);
    let mut test_config_file = PathBuf::new();
    test_config_file.push(project_root_dir);
    test_config_file.push("pg-test-config.toml");

    let config_str = fs::read_to_string(test_config_file)?;

    // Parse into a dynamic TOML Value
    let value = config_str.parse::<Table>()?;

    // Access nested fields
    if let Some(data_dir) = value
        .get("postgres")
        .and_then(|v| v.get("pg18"))
        .and_then(|v| v.get("data_dir"))
        .and_then(|v| v.as_str())
    {
        println!("Data Directory: {}", data_dir);
        Ok(data_dir.to_string())
    } else {
        Ok("".to_string())
    }
}

// typedef struct PageHeaderData
// {
// 	/* XXX LSN is member of *any* block, not only page-organized ones */
// 	PageXLogRecPtr pd_lsn;		/* LSN: next byte after last byte of xlog
// 								 * record for last change to this page */
// 	uint16		pd_checksum;	/* checksum */
// 	uint16		pd_flags;		/* flag bits, see below */
// 	LocationIndex pd_lower;		/* offset to start of free space */
// 	LocationIndex pd_upper;		/* offset to end of free space */
// 	LocationIndex pd_special;	/* offset to start of special space */
// 	uint16		pd_pagesize_version;
// 	TransactionId pd_prune_xid; /* oldest prunable XID, or zero if none */
// 	ItemIdData	pd_linp[FLEXIBLE_ARRAY_MEMBER]; /* line pointer array */
// } PageHeaderData;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_page_header() {
        use std::fs::File;
        use std::io::prelude::*;

        let data_dir = get_data_dir().unwrap();

        let mut table_file = File::open(format!("{}/base/16384/16511", data_dir)).unwrap();
        let mut page_buffer = [0; PAGE_BUFFER_SIZE];

        table_file.read_exact(&mut page_buffer).unwrap();

        let xlogid = u32::from_le_bytes(page_buffer[0..4].try_into().unwrap());
        let xrecoff = u32::from_le_bytes(page_buffer[4..8].try_into().unwrap());

        let pd_checksum = u16::from_le_bytes(page_buffer[8..10].try_into().unwrap());
        let pd_flags = u16::from_le_bytes(page_buffer[10..12].try_into().unwrap());

        let pd_lower = u16::from_le_bytes(page_buffer[12..14].try_into().unwrap());
        let pd_upper = u16::from_le_bytes(page_buffer[14..16].try_into().unwrap());
        let pd_special = u16::from_le_bytes(page_buffer[16..18].try_into().unwrap());

        let pagesize = u16::from_le_bytes(page_buffer[18..20].try_into().unwrap()) & 0xFF00;
        let version = u16::from_le_bytes(page_buffer[18..20].try_into().unwrap()) & 0x00FF;

        let pd_prune_xid = u32::from_le_bytes(page_buffer[20..24].try_into().unwrap());

        println!("xlogid {:X}", xlogid);
        println!("xrecoff {:X}", xrecoff);
        println!("checksum {:X}", pd_checksum);
        println!("pd_flags {:}", pd_flags);
        println!("pd_lower {:}", pd_lower);
        println!("pd_upper {:}", pd_upper);
        println!("pd_special {:}", pd_special);
        println!("pagesize {:}", pagesize);
        println!("version {:}", version);
        println!("pd_prune_xid {:}", pd_prune_xid);

        assert_eq!(pagesize as usize, PAGE_BUFFER_SIZE);
    }
}
