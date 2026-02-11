# PostgreSQL WAL Physical Format -- Implementation-Ready Reference

## Date: 2026-02-11

---

## Table of Contents

1. [WAL File Organization](#1-wal-file-organization) — Directory structure, segment naming, segment size, pages, timeline
2. [LSN Arithmetic](#2-lsn-log-sequence-number-arithmetic) — LSN definition, LSN→segment+offset, LSN→filename, advancing, validity
3. [WAL Page Layout](#3-wal-page-layout) — Short header (24B), long header (40B), page flags, XLOG_PAGE_MAGIC version detection
4. [WAL Record Layout](#4-wal-record-layout-xlogrecord) — XLogRecord (24B), xl_info interpretation, CRC, max size
5. [Overall Record Structure](#5-overall-record-structure) — Two-phase layout: headers first, then data
6. [Block Reference Headers](#6-block-reference-headers) — Base header (4B), fork_flags, FPI header, compress header, RelFileLocator, BlockNumber
7. [Special Block IDs and Main Data Headers](#7-special-block-ids-and-main-data-headers) — Reserved IDs, short/long data headers, decoding algorithm
8. [Continuation Records](#8-continuation-records-records-spanning-pages) — How records span pages, reading algorithm, next record
9. [Full-Page Image (FPI) Restoration](#9-full-page-image-fpi-restoration) — Storage format, restore algorithm, when FPIs are generated
10. [Resource Manager IDs](#10-resource-manager-ids) — All rmgr IDs, critical ones for pg_arrow
11. [Heap WAL Record Types (RM_HEAP_ID)](#11-heap-wal-record-types-rm_heap_id--10) — INSERT, DELETE, UPDATE, HOT_UPDATE, LOCK, CONFIRM, INPLACE with exact byte layouts
12. [Heap2 WAL Record Types (RM_HEAP2_ID)](#12-heap2-wal-record-types-rm_heap2_id--9) — MULTI_INSERT, PRUNE (PG17+ unified), VISIBLE, LOCK_UPDATED, NEW_CID, REWRITE
13. [Infobits Conversion](#13-infobits-conversion) — fix_infomask_from_infobits mapping
14. [Scanning WAL for a Specific Page](#14-scanning-wal-for-records-affecting-a-specific-page) — Algorithm, applying records, LSN check, performance
15. [Version Differences (PG14-master)](#15-version-differences-pg14-through-pg-master) — Per-version changes, breaking changes summary, opcode shifts, version dispatch code
16. [Complete Rust Struct Definitions](#16-complete-rust-struct-definitions) — All types, constants, parse implementations
17. [ForkNumber Reference](#17-forknumber-reference)
18. [Key Source Files Reference](#18-key-source-files-reference)
19. [Existing Implementations and References](#19-existing-implementations-and-references) — pg_walinspect, pg_waldump, Neon, key papers
20. [Implementation Strategy for pg_arrow](#20-decision-implementation-strategy-for-pg_arrow) — Recommended approach, future considerations

---

### Context

pg_arrow needs a WAL parser to implement read consistency. When reading heap
pages from disk, those pages may be stale (behind the current WAL position).
The WAL parser must scan WAL records between a page's LSN and a target LSN,
find records that modify the page, and replay them. This document provides
the exact binary layouts needed to implement that parser in Rust.

**Endianness**: WAL files use the server's native byte order. On x86_64 and
ARM64 (the two targets that matter), this is little-endian. Use `from_ne_bytes`
when reading WAL from the same machine.

**Alignment**: PostgreSQL uses `MAXALIGN` (8 bytes on 64-bit systems, 4 bytes
on 32-bit). WAL records are MAXALIGN'd. Internal fields within records are
generally *not* aligned -- they are packed and must be read via memcpy or
byte-slice parsing, not pointer casts.

---

## 1. WAL File Organization

### 1.1 Directory Structure

WAL files live in `$PGDATA/pg_wal/`. Each file is called a "segment".

### 1.2 Segment Naming

Segment filenames are exactly 24 hex characters: `TTTTTTTTSSSSSSSSSSSSSSSS`

```
Format: sprintf("%08X%08X%08X", timeline_id, log_id, seg_id)

Where:
  timeline_id : uint32 (TimeLineID)
  log_id      : uint32 = segment_number / segments_per_xlog_id
  seg_id      : uint32 = segment_number % segments_per_xlog_id

  segments_per_xlog_id = 0x100000000 / wal_segment_size
                       = 0x100000000 / 16MB = 256  (for default 16MB segments)
```

Example: `000000010000000000000003`
- Timeline 1, segment number 3

**Source**: `XLogFileName()` in `src/include/access/xlog_internal.h`

### 1.3 Segment Size

- Default: 16 MB (16,777,216 bytes) -- `wal_segment_size`
- Configurable at initdb time: must be power-of-2, range 1MB to 1GB
- Read from `pg_control` or the long page header's `xlp_seg_size` field

### 1.4 WAL Pages Within a Segment

Each segment is divided into 8KB pages (XLOG_BLCKSZ = 8192). Every page has
a page header. The default 16MB segment contains 2048 pages.

```
Segment (16MB):
  Page 0: [LongPageHeader (40B)] [record data...]
  Page 1: [ShortPageHeader (24B)] [record data...]
  Page 2: [ShortPageHeader (24B)] [record data...]
  ...
  Page 2047: [ShortPageHeader (24B)] [record data...]
```

### 1.5 Timeline Concept

TimeLineID (uint32) distinguishes WAL histories after point-in-time recovery.
Normal operation stays on timeline 1. Each PITR or standby promotion increments
the timeline. Timeline history files (`TTTTTTTT.history`) record the lineage.

For pg_arrow's WAL parser, always read the timeline from the page header's
`xlp_tli` field and validate it matches what you expect.

---

## 2. LSN (Log Sequence Number) Arithmetic

### 2.1 LSN Definition

```
XLogRecPtr = uint64
```

An LSN is a 64-bit byte offset into the abstract, continuous WAL stream.
Displayed as `%X/%08X` (upper 32 bits / lower 32 bits), e.g., `0/1A000060`.

**Source**: `src/include/access/xlogdefs.h`

### 2.2 LSN to Segment + Offset

```rust
// Given: lsn (u64), wal_segment_size (u32, default 16MB = 0x1000000)

let segment_number: u64 = lsn / wal_segment_size as u64;
let segment_offset: u64 = lsn & (wal_segment_size as u64 - 1);

// Within the segment, which page?
let page_number: u64 = segment_offset / XLOG_BLCKSZ as u64;  // XLOG_BLCKSZ=8192
let page_offset: u64 = segment_offset % XLOG_BLCKSZ as u64;
```

**Source**: `XLByteToSeg`, `XLogSegmentOffset` in `src/include/access/xlog_internal.h`

### 2.3 LSN to Segment Filename

```rust
let segments_per_xlog_id: u64 = 0x100000000u64 / wal_segment_size as u64;
let log_id: u32 = (segment_number / segments_per_xlog_id) as u32;
let seg_id: u32 = (segment_number % segments_per_xlog_id) as u32;
let filename = format!("{:08X}{:08X}{:08X}", timeline_id, log_id, seg_id);
```

### 2.4 Advancing Past a Record

```rust
// After reading a record at lsn with xl_tot_len:
let next_record_lsn = lsn + maxalign(xl_tot_len as u64);

// MAXALIGN on 64-bit: round up to multiple of 8
fn maxalign(len: u64) -> u64 {
    (len + 7) & !7
}
```

Special case: XLOG_SWITCH records logically extend to the end of the segment.

### 2.5 Validity Check

An LSN is valid (points to actual record data, not a page header) if:
```rust
fn xrec_off_is_valid(lsn: u64) -> bool {
    let page_offset = lsn % XLOG_BLCKSZ as u64;
    page_offset >= SIZE_OF_XLOG_SHORT_PHD as u64  // >= 24
}
```

---

## 3. WAL Page Layout

Every 8KB page in a WAL segment starts with a page header. The first page
of each segment file uses a "long" header; all other pages use a "short" header.

### 3.1 Short Page Header: XLogPageHeaderData (24 bytes)

```
Offset  Size  Field           Type          Description
------  ----  -----           ----          -----------
 0      2     xlp_magic       uint16        Magic number (version indicator)
 2      2     xlp_info        uint16        Flag bits
 4      4     xlp_tli         uint32        TimeLineID of first record on page
 8      8     xlp_pageaddr    uint64        LSN address of this page's start
16      4     xlp_rem_len     uint32        Remaining bytes from previous page's record
------
20 bytes raw, MAXALIGN'd to 24 bytes
```

`SizeOfXLogShortPHD = MAXALIGN(sizeof(XLogPageHeaderData)) = 24`

**Source**: `src/include/access/xlog_internal.h`

### 3.2 Long Page Header: XLogLongPageHeaderData (40 bytes)

The first page of every WAL segment file has the long header. It extends the
short header with additional identification fields.

```
Offset  Size  Field              Type          Description
------  ----  -----              ----          -----------
 0      20    std                               Short page header fields (see above)
20      8     xlp_sysid          uint64        System identifier from pg_control
28      4     xlp_seg_size       uint32        WAL segment size (cross-check)
32      4     xlp_xlog_blcksz    uint32        WAL block size (cross-check, = 8192)
------
36 bytes raw, MAXALIGN'd to 40 bytes
```

`SizeOfXLogLongPHD = MAXALIGN(sizeof(XLogLongPageHeaderData)) = 40`

### 3.3 Page Header Flags (xlp_info)

```
XLP_FIRST_IS_CONTRECORD          = 0x0001  // First data on page is continuation of previous record
XLP_LONG_HEADER                  = 0x0002  // This page has a long header
XLP_BKP_REMOVABLE                = 0x0004  // Backup blocks on this page are optional
XLP_FIRST_IS_OVERWRITE_CONTRECORD= 0x0008  // Replaces a missing contrecord
XLP_ALL_FLAGS                    = 0x000F
```

### 3.4 Determining Header Size

```rust
fn page_header_size(xlp_info: u16) -> usize {
    if xlp_info & XLP_LONG_HEADER != 0 {
        40  // SizeOfXLogLongPHD
    } else {
        24  // SizeOfXLogShortPHD
    }
}
```

### 3.5 XLOG_PAGE_MAGIC Values (Version Detection)

| PG Version | XLOG_PAGE_MAGIC | Notes |
|------------|----------------|-------|
| PG 14      | 0xD110         |       |
| PG 15      | 0xD113         | Added LZ4/ZSTD FPI compression |
| PG 16      | 0xD114         | RelFileLocator replaces RelFileNode |
| PG 17      | 0xD116         | Pruning/freezing WAL record changes |
| PG 18      | 0xD118         | xl_heap_prune flags becomes uint8+uint8 (reason+flags) |
| PG master  | 0xD11A         | xl_heap_prune flags merged to uint16, VM flags in prune |

**Source**: `XLOG_PAGE_MAGIC` in `src/include/access/xlog_internal.h`

---

## 4. WAL Record Layout: XLogRecord

WAL records begin on MAXALIGN boundaries within pages (after the page header).
A record can span multiple pages via the continuation mechanism.

### 4.1 Record Header: XLogRecord (24 bytes)

```
Offset  Size  Field      Type            Description
------  ----  -----      ----            -----------
 0      4     xl_tot_len uint32          Total length of ENTIRE record (header + all data)
 4      4     xl_xid     uint32          Transaction ID (TransactionId)
 8      8     xl_prev    uint64          LSN of previous record (XLogRecPtr)
16      1     xl_info    uint8           Info flags: low 4 bits internal, high 4 bits rmgr-specific
17      1     xl_rmid    uint8           Resource manager ID (RmgrId)
18      2     (padding)  -               Must be zero
20      4     xl_crc     uint32          CRC-32C of the entire record
------
24 bytes total
```

`SizeOfXLogRecord = 24` (offsetof(xl_crc) + sizeof(pg_crc32c))

**Source**: `src/include/access/xlogrecord.h`

### 4.2 xl_info Interpretation

```
Low 4 bits  (xl_info & 0x0F):  Internal flags
  XLR_SPECIAL_REL_UPDATE = 0x01  // Record modifies relation files in special ways
  XLR_CHECK_CONSISTENCY  = 0x02  // Consistency check record

High 4 bits (xl_info & 0xF0):  Resource-manager-specific opcode
  For heap: encodes the operation type (INSERT=0x00, DELETE=0x10, etc.)
  XLOG_HEAP_INIT_PAGE = 0x80 is ORed when the page is re-initialized
```

### 4.3 CRC Calculation

CRC-32C (Castagnoli polynomial, same as iSCSI). The CRC covers:
1. The record header with xl_crc set to 0
2. All data following the header (block headers, block data, main data)

PostgreSQL uses hardware-accelerated CRC-32C via SSE4.2 or ARM CRC extensions.
In Rust, the `crc32c` crate provides this.

### 4.4 Record Maximum Size

`XLogRecordMaxSize = 1020 * 1024 * 1024` (approximately 1GB)

---

## 5. Overall Record Structure

After the 24-byte `XLogRecord` header, the remainder of the record contains
(in order):

```
[XLogRecord header -- 24 bytes]
[Block Reference Header 0]     (XLogRecordBlockHeader + optional sub-headers)
[Block Reference Header 1]     (XLogRecordBlockHeader + optional sub-headers)
...
[Block Reference Header N]
[Main Data Header]              (XLogRecordDataHeaderShort or Long)
[Block 0 FPI data]              (full-page image bytes, if present)
[Block 0 block-specific data]   (rmgr-specific data for block 0)
[Block 1 FPI data]
[Block 1 block-specific data]
...
[Main data]                     (rmgr-specific main record payload)
```

**Key point**: The headers come first (sequentially), then all the data
payloads follow in the same order. Headers and data are separated.

**Source**: Comment at top of `src/include/access/xlogrecord.h`

---

## 6. Block Reference Headers

### 6.1 XLogRecordBlockHeader (4 bytes base)

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      1     id            uint8    Block reference ID (0..XLR_MAX_BLOCK_ID=32)
 1      1     fork_flags    uint8    Fork number (low 4 bits) + flags (high 4 bits)
 2      2     data_length   uint16   Payload bytes for this block (not counting FPI)
------
4 bytes
```

`SizeOfXLogRecordBlockHeader = 4`

### 6.2 fork_flags Interpretation

```
Low 4 bits  (fork_flags & 0x0F) = ForkNumber:
  MAIN_FORKNUM          = 0
  FSM_FORKNUM           = 1
  VISIBILITYMAP_FORKNUM = 2
  INIT_FORKNUM          = 3

High 4 bits (fork_flags & 0xF0) = Flags:
  BKPBLOCK_HAS_IMAGE  = 0x10   // A full-page image follows
  BKPBLOCK_HAS_DATA   = 0x20   // Block-specific data follows
  BKPBLOCK_WILL_INIT  = 0x40   // Page will be re-initialized during redo
  BKPBLOCK_SAME_REL   = 0x80   // RelFileLocator is same as previous block ref
```

### 6.3 What Follows the Base Header

After the 4-byte base, additional fields appear conditionally:

```
[XLogRecordBlockHeader -- 4 bytes, always]

IF BKPBLOCK_HAS_IMAGE:
  [XLogRecordBlockImageHeader -- 5 bytes]
  IF BKPIMAGE_COMPRESSED() AND BKPIMAGE_HAS_HOLE:
    [XLogRecordBlockCompressHeader -- 2 bytes]

IF NOT BKPBLOCK_SAME_REL:
  [RelFileLocator -- 12 bytes]

[BlockNumber -- 4 bytes, always]
```

### 6.4 XLogRecordBlockImageHeader (5 bytes)

Present only when `BKPBLOCK_HAS_IMAGE` is set in fork_flags.

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      2     length        uint16   Number of page image bytes stored
 2      2     hole_offset   uint16   Offset of the "hole" (zero-filled region)
 4      1     bimg_info     uint8    Flag bits
------
5 bytes
```

`SizeOfXLogRecordBlockImageHeader = 5`

### 6.5 bimg_info Flags

```
BKPIMAGE_HAS_HOLE       = 0x01  // Image has a "hole" of zeros removed
BKPIMAGE_APPLY          = 0x02  // Image should be restored during replay
BKPIMAGE_COMPRESS_PGLZ  = 0x04  // Compressed with pglz
BKPIMAGE_COMPRESS_LZ4   = 0x08  // Compressed with LZ4 (PG15+)
BKPIMAGE_COMPRESS_ZSTD  = 0x10  // Compressed with zstd (PG15+)
```

Compression check macro:
```rust
fn bkpimage_is_compressed(bimg_info: u8) -> bool {
    (bimg_info & (0x04 | 0x08 | 0x10)) != 0
}
```

### 6.6 XLogRecordBlockCompressHeader (2 bytes)

Present only when the image is compressed AND has a hole.

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      2     hole_length   uint16   Number of bytes in the "hole"
------
2 bytes
```

When not compressed, hole_length is implicitly `BLCKSZ - bimg_len`.
When compressed but no hole, hole_length is 0.

### 6.7 RelFileLocator (12 bytes)

Present when `BKPBLOCK_SAME_REL` is NOT set. This identifies the physical
relation file.

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      4     spcOid        uint32   Tablespace OID (Oid)
 4      4     dbOid         uint32   Database OID (Oid)
 8      4     relNumber     uint32   Relation file number (RelFileNumber = Oid)
------
12 bytes
```

**IMPORTANT version difference**: Before PG16, this was `RelFileNode` with fields
`(spcNode, dbNode, relNode)` -- same layout, different field names. PG16+
renamed to `RelFileLocator` with `(spcOid, dbOid, relNumber)`.

**Source**: `src/include/storage/relfilelocator.h`

### 6.8 BlockNumber (4 bytes)

Always present after the RelFileLocator (or after the image/compress headers if
BKPBLOCK_SAME_REL is set).

```
BlockNumber = uint32
```

**Source**: `src/include/storage/block.h`

### 6.9 Maximum Block Header Size

```
MaxSizeOfXLogRecordBlockHeader =
    4  (base header)
  + 5  (image header)
  + 2  (compress header)
  + 12 (RelFileLocator)
  + 4  (BlockNumber)
  = 27 bytes
```

---

## 7. Special Block IDs and Main Data Headers

### 7.1 Reserved Block IDs

```
XLR_MAX_BLOCK_ID          = 32   // Maximum block reference ID for actual blocks
XLR_BLOCK_ID_DATA_SHORT   = 255  // Main data, short header (length < 256)
XLR_BLOCK_ID_DATA_LONG    = 254  // Main data, long header (length >= 256)
XLR_BLOCK_ID_ORIGIN       = 253  // Replication origin ID follows (uint16)
XLR_BLOCK_ID_TOPLEVEL_XID = 252  // Top-level transaction ID follows (uint32)
```

### 7.2 XLogRecordDataHeaderShort (2 bytes)

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      1     id            uint8    = 255 (XLR_BLOCK_ID_DATA_SHORT)
 1      1     data_length   uint8    Length of main data (0-255)
------
2 bytes
```

### 7.3 XLogRecordDataHeaderLong (5 bytes)

```
Offset  Size  Field         Type     Description
------  ----  -----         ----     -----------
 0      1     id            uint8    = 254 (XLR_BLOCK_ID_DATA_LONG)
 1      4     data_length   uint32   Length of main data (unaligned!)
------
5 bytes
```

### 7.4 Decoding Algorithm for Headers

The decode loop (from `DecodeXLogRecord` in `xlogreader.c`):

```rust
let mut ptr = record_start + 24; // skip XLogRecord header
let mut remaining = xl_tot_len - 24;
let mut datatotal: u32 = 0;
let mut last_rlocator: Option<RelFileLocator> = None;

while remaining > datatotal {
    let block_id = read_u8(&mut ptr);
    remaining -= 1;

    match block_id {
        255 => { // XLR_BLOCK_ID_DATA_SHORT
            let len = read_u8(&mut ptr) as u32;
            remaining -= 1;
            main_data_len = len;
            datatotal += len;
            break; // main data is always last
        }
        254 => { // XLR_BLOCK_ID_DATA_LONG
            let len = read_u32_unaligned(&mut ptr);
            remaining -= 4;
            main_data_len = len;
            datatotal += len;
            break;
        }
        253 => { // XLR_BLOCK_ID_ORIGIN
            let origin = read_u16_unaligned(&mut ptr);
            remaining -= 2;
        }
        252 => { // XLR_BLOCK_ID_TOPLEVEL_XID
            let xid = read_u32_unaligned(&mut ptr);
            remaining -= 4;
        }
        0..=32 => {
            // Block reference header
            let fork_flags = read_u8(&mut ptr);
            let data_length = read_u16_unaligned(&mut ptr);
            remaining -= 3;

            let forknum = fork_flags & 0x0F;
            let has_image = (fork_flags & 0x10) != 0;
            let has_data = (fork_flags & 0x20) != 0;

            datatotal += data_length as u32;

            if has_image {
                let bimg_len = read_u16_unaligned(&mut ptr);
                let hole_offset = read_u16_unaligned(&mut ptr);
                let bimg_info = read_u8(&mut ptr);
                remaining -= 5;

                let hole_length = if bkpimage_is_compressed(bimg_info) {
                    if (bimg_info & 0x01) != 0 { // BKPIMAGE_HAS_HOLE
                        let hl = read_u16_unaligned(&mut ptr);
                        remaining -= 2;
                        hl
                    } else {
                        0
                    }
                } else {
                    (BLCKSZ as u16) - bimg_len
                };

                datatotal += bimg_len as u32;
            }

            if (fork_flags & 0x80) == 0 { // NOT BKPBLOCK_SAME_REL
                let rlocator = read_relfilelocator(&mut ptr); // 12 bytes
                remaining -= 12;
                last_rlocator = Some(rlocator);
            }
            // else: reuse last_rlocator

            let blkno = read_u32_unaligned(&mut ptr); // BlockNumber
            remaining -= 4;
        }
        _ => {
            // Invalid block_id
            return Err(...);
        }
    }
}
assert_eq!(remaining, datatotal);
```

After all headers are parsed, the data payloads follow in this order:
1. For each block ref (in block_id order): FPI image bytes, then block data bytes
2. Main data bytes (last)

---

## 8. Continuation Records (Records Spanning Pages)

### 8.1 How It Works

A WAL record can be larger than the remaining space on a page. When this
happens:

1. The record starts on the current page (as much as fits)
2. The next page's header has `XLP_FIRST_IS_CONTRECORD` set in `xlp_info`
3. The next page's `xlp_rem_len` field contains the remaining bytes
4. Continuation data starts immediately after the page header
5. This can chain across multiple pages for very large records

### 8.2 Reading Algorithm

```rust
fn read_record(lsn: u64, segment_data: &[u8]) -> Vec<u8> {
    let page_start = lsn & !(XLOG_BLCKSZ as u64 - 1);
    let page_offset = (lsn % XLOG_BLCKSZ as u64) as usize;

    // Read page header to determine header size
    let page_hdr = parse_page_header(&segment_data[page_start as usize..]);
    let hdr_size = page_header_size(page_hdr.xlp_info);

    // Read XLogRecord header (at least first 24 bytes)
    let record_start = page_start as usize + page_offset;
    let xl_tot_len = read_u32(&segment_data[record_start..]);

    let space_on_page = XLOG_BLCKSZ - page_offset;

    if xl_tot_len as usize <= space_on_page {
        // Record fits on one page -- simple case
        return segment_data[record_start..record_start + xl_tot_len as usize].to_vec();
    }

    // Record spans pages -- must reassemble
    let mut buf = Vec::with_capacity(xl_tot_len as usize);

    // Copy what's on the first page
    buf.extend_from_slice(&segment_data[record_start..page_start as usize + XLOG_BLCKSZ]);

    let mut target_page = page_start + XLOG_BLCKSZ as u64;
    let mut got = space_on_page;

    while got < xl_tot_len as usize {
        let cont_hdr = parse_page_header(&segment_data[target_page as usize..]);

        assert!(cont_hdr.xlp_info & XLP_FIRST_IS_CONTRECORD != 0);
        assert_eq!(cont_hdr.xlp_rem_len as usize, xl_tot_len as usize - got);

        let cont_hdr_size = page_header_size(cont_hdr.xlp_info);
        let cont_data_start = target_page as usize + cont_hdr_size;

        let available = XLOG_BLCKSZ - cont_hdr_size;
        let needed = xl_tot_len as usize - got;
        let take = needed.min(available);

        buf.extend_from_slice(&segment_data[cont_data_start..cont_data_start + take]);
        got += take;
        target_page += XLOG_BLCKSZ as u64;
    }

    // After last continuation, next record starts at MAXALIGN after cont data
    buf
}
```

### 8.3 Finding Next Record After Continuation

If the record was entirely on one page:
```
next_lsn = record_lsn + MAXALIGN(xl_tot_len)
```

If the record spanned pages, next_lsn is on the last continuation page:
```
next_lsn = last_cont_page_addr + page_header_size + MAXALIGN(xlp_rem_len)
```

---

## 9. Full-Page Image (FPI) Restoration

When a WAL record includes a full-page image (BKPBLOCK_HAS_IMAGE), it contains
a snapshot of the entire page. This is used for crash recovery to avoid
torn-page problems.

### 9.1 FPI Storage Format

The page image stored in WAL has the "hole" removed. The "hole" is a region
of consecutive zero bytes in the middle of the page (typically the gap between
line pointers and tuple data in heap pages).

```
Stored image bytes: [bytes before hole] [bytes after hole]
Stored length: BLCKSZ - hole_length  (if not compressed)
```

### 9.2 Restoring an FPI (from RestoreBlockImage in xlogreader.c)

```rust
fn restore_block_image(
    bkp_image: &[u8],     // The stored image bytes
    bimg_len: u16,         // Length of stored image
    bimg_info: u8,         // bimg_info flags
    hole_offset: u16,      // Offset of hole
    hole_length: u16,      // Length of hole
) -> Result<[u8; BLCKSZ], Error> {
    let mut page = [0u8; BLCKSZ];
    let decompressed: Vec<u8>;

    let src = if bkpimage_is_compressed(bimg_info) {
        let decomp_size = BLCKSZ - hole_length as usize;
        decompressed = match bimg_info {
            i if (i & BKPIMAGE_COMPRESS_PGLZ) != 0 =>
                pglz_decompress(bkp_image, decomp_size)?,
            i if (i & BKPIMAGE_COMPRESS_LZ4) != 0 =>
                lz4_decompress(bkp_image, decomp_size)?,
            i if (i & BKPIMAGE_COMPRESS_ZSTD) != 0 =>
                zstd_decompress(bkp_image, decomp_size)?,
            _ => return Err(Error::UnknownCompression),
        };
        &decompressed
    } else {
        bkp_image
    };

    if hole_length == 0 {
        // No hole -- image is the entire page
        page.copy_from_slice(&src[..BLCKSZ]);
    } else {
        // Copy bytes before hole
        page[..hole_offset as usize].copy_from_slice(&src[..hole_offset as usize]);
        // hole_offset..hole_offset+hole_length is already zero (from init)
        // Copy bytes after hole
        let after_hole = hole_offset as usize + hole_length as usize;
        page[after_hole..].copy_from_slice(&src[hole_offset as usize..]);
    }

    Ok(page)
}
```

### 9.3 When FPIs Are Generated

- First modification of a page after a checkpoint (to protect against torn pages)
- When `wal_log_hints` is enabled and hint bits are set
- When `wal_consistency_checking` is enabled
- On `full_page_writes = on` (default)

For pg_arrow: When you encounter an FPI for your target page, you can use it
directly as the page state -- it's a complete snapshot. No need to replay
individual tuple operations when an FPI covers the same LSN range.

---

## 10. Resource Manager IDs

The `xl_rmid` field identifies which subsystem generated the record. The IDs
are assigned by position in `rmgrlist.h`.

### 10.1 Relevant Resource Manager IDs

```
RM_XLOG_ID       = 0   // Checkpoint, switch, parameter change, etc.
RM_XACT_ID       = 1   // Transaction commit/abort
RM_SMGR_ID       = 2   // Storage manager (create/truncate/extend)
RM_CLOG_ID       = 3   // Commit log
RM_DBASE_ID       = 4   // Database create/drop
RM_TBLSPC_ID      = 5   // Tablespace
RM_MULTIXACT_ID   = 6   // MultiXact
RM_RELMAP_ID       = 7   // Relation map
RM_STANDBY_ID      = 8   // Standby-related
RM_HEAP2_ID        = 9   // Heap operations (second set)
RM_HEAP_ID         = 10  // Heap operations (primary set)
RM_BTREE_ID        = 11  // B-tree index
RM_HASH_ID         = 12  // Hash index
RM_GIN_ID          = 13  // GIN index
RM_GIST_ID         = 14  // GiST index
RM_SEQ_ID          = 15  // Sequences
RM_SPGIST_ID       = 16  // SP-GiST index
RM_BRIN_ID         = 17  // BRIN index
RM_COMMIT_TS_ID    = 18  // Commit timestamps
RM_REPLORIGIN_ID   = 19  // Replication origin
RM_GENERIC_ID      = 20  // Generic WAL
RM_LOGICALMSG_ID   = 21  // Logical messages
```

**For pg_arrow, the critical ones are RM_HEAP_ID (10) and RM_HEAP2_ID (9).**

**Source**: `src/include/access/rmgrlist.h`, `src/include/access/rmgr.h`

---

## 11. Heap WAL Record Types (RM_HEAP_ID = 10)

The opcode is in `xl_info & XLOG_HEAP_OPMASK` (bits 4-6, i.e., `xl_info & 0x70`).
Bit 7 (`XLOG_HEAP_INIT_PAGE = 0x80`) indicates the page should be re-initialized.

**Source**: `src/include/access/heapam_xlog.h`

### 11.1 XLOG_HEAP_INSERT (0x00)

#### Main data: xl_heap_insert (3 bytes)

```
Offset  Size  Field    Type            Description
------  ----  -----    ----            -----------
 0      2     offnum   uint16          Offset number where tuple was inserted (OffsetNumber)
 2      1     flags    uint8           XLH_INSERT_* flags
------
3 bytes
```

#### Block 0 data: xl_heap_header (5 bytes) + tuple data

```
[xl_heap_header]
  Offset  Size  Field        Type     Description
   0      2     t_infomask2  uint16
   2      2     t_infomask   uint16
   4      1     t_hoff       uint8
[tuple data bytes follow -- starting from t_hoff offset into a HeapTupleHeader,
 which means the null bitmap + user data, excluding the first 23 bytes of
 HeapTupleHeaderData that are reconstructed from WAL context]
```

#### Reconstruction during replay:

1. Allocate a zeroed HeapTupleHeaderData (23 bytes)
2. Copy the tuple data (from block 0 data, after xl_heap_header) starting at
   offset 23 (SizeofHeapTupleHeader)
3. Set t_infomask2 and t_infomask from xl_heap_header
4. Set t_hoff from xl_heap_header
5. Set t_xmin = xl_xid (from XLogRecord header)
6. Set t_cmin = FirstCommandId (0)
7. Set t_ctid = (blkno, offnum)
8. Insert into page at offnum via PageAddItem

If `XLOG_HEAP_INIT_PAGE` is set, the page is zeroed and re-initialized first.

#### Insert flags:

```
XLH_INSERT_ALL_VISIBLE_CLEARED  = 0x01  // PD_ALL_VISIBLE was cleared
XLH_INSERT_LAST_IN_MULTI        = 0x02  // Last in multi-insert batch
XLH_INSERT_IS_SPECULATIVE       = 0x04  // Speculative insertion
XLH_INSERT_CONTAINS_NEW_TUPLE   = 0x08  // Tuple data included even with FPI
XLH_INSERT_ON_TOAST_RELATION    = 0x10  // Insert is on a TOAST relation
XLH_INSERT_ALL_FROZEN_SET       = 0x20  // Page marked all-frozen
```

### 11.2 XLOG_HEAP_DELETE (0x10)

#### Main data: xl_heap_delete (8 bytes)

```
Offset  Size  Field          Type            Description
------  ----  -----          ----            -----------
 0      4     xmax           uint32          Transaction ID that deleted the tuple
 4      2     offnum         uint16          Offset number of deleted tuple
 6      1     infobits_set   uint8           Infomask bits to set
 7      1     flags          uint8           XLH_DELETE_* flags
------
8 bytes
```

#### Replay:
1. Find tuple at (blkno, offnum) via block ref 0
2. Clear HEAP_XMAX_BITS and HEAP_MOVED from t_infomask
3. Clear HEAP_KEYS_UPDATED from t_infomask2
4. Set infomask bits from infobits_set (see fix_infomask_from_infobits below)
5. Set t_xmax = xmax (unless XLH_DELETE_IS_SUPER, which sets t_xmin = InvalidXid)
6. Set t_cmax = FirstCommandId
7. Set t_ctid = (blkno, offnum) -- self-pointing (unless partition move)

#### Delete flags:

```
XLH_DELETE_ALL_VISIBLE_CLEARED = 0x01
XLH_DELETE_CONTAINS_OLD_TUPLE  = 0x02
XLH_DELETE_CONTAINS_OLD_KEY    = 0x04
XLH_DELETE_IS_SUPER            = 0x08  // Superdelete (invalidates xmin)
XLH_DELETE_IS_PARTITION_MOVE   = 0x10  // DELETE due to partition move
```

### 11.3 XLOG_HEAP_UPDATE (0x20) and XLOG_HEAP_HOT_UPDATE (0x40)

These share the same structure; HOT_UPDATE indicates the update was a
Heap-Only Tuple optimization (new tuple on same page, no index update needed).

#### Main data: xl_heap_update (14 bytes)

```
Offset  Size  Field              Type            Description
------  ----  -----              ----            -----------
 0      4     old_xmax           uint32          Xmax to set on old tuple
 4      2     old_offnum         uint16          Old tuple's offset number
 6      1     old_infobits_set   uint8           Infomask bits for old tuple
 7      1     flags              uint8           XLH_UPDATE_* flags
 8      4     new_xmax           uint32          Xmax for new tuple (usually 0)
12      2     new_offnum         uint16          New tuple's offset number
------
14 bytes
```

#### Block references:
- Block 0: The page with the NEW tuple
- Block 1 (optional): The page with the OLD tuple, if different from block 0.
  If block 1 is absent, old and new are on the same page.

#### Block 0 data (new tuple):

```
[optional uint16 prefix_len]   -- if XLH_UPDATE_PREFIX_FROM_OLD
[optional uint16 suffix_len]   -- if XLH_UPDATE_SUFFIX_FROM_OLD
[xl_heap_header -- 5 bytes]
[new tuple data bytes]
```

When PREFIX_FROM_OLD or SUFFIX_FROM_OLD is set, the new tuple data is a delta:
the prefix/suffix bytes are copied from the old tuple during replay.

#### Update flags:

```
XLH_UPDATE_OLD_ALL_VISIBLE_CLEARED = 0x01
XLH_UPDATE_NEW_ALL_VISIBLE_CLEARED = 0x02
XLH_UPDATE_CONTAINS_OLD_TUPLE      = 0x04
XLH_UPDATE_CONTAINS_OLD_KEY        = 0x08
XLH_UPDATE_CONTAINS_NEW_TUPLE      = 0x10
XLH_UPDATE_PREFIX_FROM_OLD          = 0x20
XLH_UPDATE_SUFFIX_FROM_OLD          = 0x40
```

### 11.4 XLOG_HEAP_TRUNCATE (0x30)

Truncation of heap relations. Not relevant for per-page replay (affects whole
relations).

### 11.5 XLOG_HEAP_CONFIRM (0x50)

Confirmation of a speculative insertion.

#### Main data: xl_heap_confirm (2 bytes)

```
Offset  Size  Field    Type     Description
 0      2     offnum   uint16   Confirmed tuple's offset
```

### 11.6 XLOG_HEAP_LOCK (0x60)

Row locking (SELECT FOR UPDATE/SHARE).

#### Main data: xl_heap_lock (8 bytes)

```
Offset  Size  Field          Type     Description
 0      4     xmax           uint32   Lock holder's transaction ID (or MultiXactId)
 4      2     offnum         uint16   Locked tuple's offset
 6      1     infobits_set   uint8
 7      1     flags          uint8    XLH_LOCK_* flags
```

### 11.7 XLOG_HEAP_INPLACE (0x70)

In-place update (used for system catalog updates that don't change tuple size).

#### Main data: xl_heap_inplace (variable)

```
Offset  Size  Field                    Type     Description
 0      2     offnum                   uint16
 2      4     dbId                     uint32
 6      4     tsId                     uint32
10      1     relcacheInitFileInval    bool
11      4     nmsgs                    int32
15      var   msgs[]                   SharedInvalidationMessage[]
```

---

## 12. Heap2 WAL Record Types (RM_HEAP2_ID = 9)

### 12.1 XLOG_HEAP2_MULTI_INSERT (0x50)

Batch insertion of multiple tuples into a single page (used by COPY).

#### Main data: xl_heap_multi_insert (variable)

```
Offset  Size  Field      Type            Description
------  ----  -----      ----            -----------
 0      1     flags      uint8           XLH_INSERT_* flags
 1      2     ntuples    uint16          Number of tuples
 3      var   offsets[]  uint16[]        Offset numbers (omitted if INIT_PAGE)
```

`SizeOfHeapMultiInsert = 3` (base, before offsets array)

#### Block 0 data: Array of xl_multi_insert_tuple + tuple data

Each tuple in the block data:

```
[SHORTALIGN padding to align to 2-byte boundary]
[xl_multi_insert_tuple -- 7 bytes]
  Offset  Size  Field        Type     Description
   0      2     datalen      uint16   Size of tuple data following
   2      2     t_infomask2  uint16
   4      2     t_infomask   uint16
   6      1     t_hoff       uint8
[tuple data -- datalen bytes]
```

Note: tuples are SHORTALIGN'd (2-byte aligned), not MAXALIGN'd.

### 12.2 XLOG_HEAP2_PRUNE_ON_ACCESS (0x10), PRUNE_VACUUM_SCAN (0x20), PRUNE_VACUUM_CLEANUP (0x30)

**PG 17+ unified pruning/freezing records.** These three opcodes share the same
structure; they differ only in the reason the record was generated.

#### Main data: xl_heap_prune

**PG18 (REL_18_STABLE):**
```
Offset  Size  Field    Type     Description
 0      1     reason   uint8    Why (on-access, vacuum scan, vacuum cleanup)
 1      1     flags    uint8    XLHP_* flags
------
2 bytes
```

**PG master (latest):**
```
Offset  Size  Field    Type     Description
 0      2     flags    uint16   XLHP_* flags (reason merged into flags, VM bits added)
------
2 bytes
```

If `XLHP_HAS_CONFLICT_HORIZON` is set, a `TransactionId` (4 bytes, unaligned)
follows immediately after xl_heap_prune in the main data.

#### XLHP_* flags:

```
XLHP_IS_CATALOG_REL         = 1 << 1
XLHP_CLEANUP_LOCK           = 1 << 2
XLHP_HAS_CONFLICT_HORIZON   = 1 << 3
XLHP_HAS_FREEZE_PLANS       = 1 << 4
XLHP_HAS_REDIRECTIONS       = 1 << 5
XLHP_HAS_DEAD_ITEMS         = 1 << 6
XLHP_HAS_NOW_UNUSED_ITEMS   = 1 << 7

// PG master only (when flags is uint16):
XLHP_VM_ALL_VISIBLE          = 1 << 8
XLHP_VM_ALL_FROZEN           = 1 << 9
```

#### Block 0 data (sub-records in order of flag bits):

1. If `XLHP_HAS_FREEZE_PLANS`:
   ```
   xlhp_freeze_plans:
     uint16 nplans
     [2 bytes padding]
     xlhp_freeze_plan[nplans]:
       uint32 xmax           (TransactionId)
       uint16 t_infomask2
       uint16 t_infomask
       uint8  frzflags
       uint16 ntuples
   ```

2. If `XLHP_HAS_REDIRECTIONS`:
   ```
   xlhp_prune_items:
     uint16 ntargets (= nredirected)
     OffsetNumber[2 * ntargets]   // pairs: (from, to)
   ```

3. If `XLHP_HAS_DEAD_ITEMS`:
   ```
   xlhp_prune_items:
     uint16 ntargets (= ndead)
     OffsetNumber[ntargets]
   ```

4. If `XLHP_HAS_NOW_UNUSED_ITEMS`:
   ```
   xlhp_prune_items:
     uint16 ntargets (= nunused)
     OffsetNumber[ntargets]
   ```

5. Freeze offset array:
   ```
   OffsetNumber[sum of all plan.ntuples]
   ```
   These are the offsets of tuples to freeze, in the same order as the plans.

### 12.3 XLOG_HEAP2_VISIBLE (0x40)

Sets visibility map bit for a page.

#### Main data: xl_heap_visible (5 bytes)

```
Offset  Size  Field                       Type     Description
 0      4     snapshotConflictHorizon      uint32   TransactionId
 4      1     flags                        uint8    VM flags
```

Block 0: visibility map buffer
Block 1: heap buffer

### 12.4 XLOG_HEAP2_LOCK_UPDATED (0x60)

Locking of an already-updated tuple version.

#### Main data: xl_heap_lock_updated (8 bytes)

```
Offset  Size  Field          Type     Description
 0      4     xmax           uint32
 4      2     offnum         uint16
 6      1     infobits_set   uint8
 7      1     flags          uint8
```

### 12.5 XLOG_HEAP2_NEW_CID (0x70)

Records a new combo CID mapping for logical decoding.

### 12.6 XLOG_HEAP2_REWRITE (0x00)

Heap rewrite (used by CLUSTER, VACUUM FULL).

---

## 13. Infobits Conversion

The `infobits_set` field in delete/update/lock records encodes which infomask
bits to set on the tuple. The conversion (from `fix_infomask_from_infobits`):

```rust
fn fix_infomask_from_infobits(infobits: u8, infomask: &mut u16, infomask2: &mut u16) {
    // Clear relevant bits first
    *infomask &= !(HEAP_XMAX_IS_MULTI | HEAP_XMAX_LOCK_ONLY |
                    HEAP_XMAX_KEYSHR_LOCK | HEAP_XMAX_EXCL_LOCK);
    *infomask2 &= !HEAP_KEYS_UPDATED;

    const XLHL_XMAX_IS_MULTI: u8    = 0x01;
    const XLHL_XMAX_LOCK_ONLY: u8   = 0x02;
    const XLHL_XMAX_EXCL_LOCK: u8   = 0x04;
    const XLHL_XMAX_KEYSHR_LOCK: u8 = 0x08;
    const XLHL_KEYS_UPDATED: u8      = 0x10;

    if infobits & XLHL_XMAX_IS_MULTI != 0    { *infomask |= HEAP_XMAX_IS_MULTI; }
    if infobits & XLHL_XMAX_LOCK_ONLY != 0   { *infomask |= HEAP_XMAX_LOCK_ONLY; }
    if infobits & XLHL_XMAX_EXCL_LOCK != 0   { *infomask |= HEAP_XMAX_EXCL_LOCK; }
    if infobits & XLHL_XMAX_KEYSHR_LOCK != 0 { *infomask |= HEAP_XMAX_KEYSHR_LOCK; }
    if infobits & XLHL_KEYS_UPDATED != 0      { *infomask2 |= HEAP_KEYS_UPDATED; }
}
```

**Source**: `fix_infomask_from_infobits()` in `heapam_xlog.c`

---

## 14. Scanning WAL for Records Affecting a Specific Page

### 14.1 The Problem

Given a target `(RelFileLocator, ForkNumber, BlockNumber)` and an LSN range
`[start_lsn, end_lsn)`, find all WAL records that modify that page.

### 14.2 Algorithm

```
ALGORITHM: ScanWalForPage(target_rlocator, target_fork, target_blkno, start_lsn, end_lsn)

1. Position WAL reader at start_lsn
2. FOR each WAL record R where R.lsn < end_lsn:
   a. Decode R's block reference headers
   b. FOR each block reference B in R (block_id 0..max_block_id):
      c. IF B.rlocator == target_rlocator
         AND B.forknum == target_fork
         AND B.blkno == target_blkno:

         IF B.has_image AND B.apply_image:
            // This is a full-page image -- it replaces the entire page.
            // All prior WAL records for this page are now irrelevant.
            DISCARD all previously collected records
            COLLECT (R, block_id) as FPI_RECORD

         ELSE:
            COLLECT (R, block_id) as REDO_RECORD

3. RETURN collected records in LSN order
```

### 14.3 Applying Collected Records

```
ALGORITHM: ApplyWalToPage(page, collected_records)

FOR each record in LSN order:
  IF record is FPI_RECORD:
    page = RestoreBlockImage(record.fpi_data)
    // FPI replaces the entire page; skip prior operations
  ELSE:
    MATCH record.xl_rmid, record.xl_info:
      (RM_HEAP_ID, XLOG_HEAP_INSERT)    => replay_insert(page, record)
      (RM_HEAP_ID, XLOG_HEAP_DELETE)    => replay_delete(page, record)
      (RM_HEAP_ID, XLOG_HEAP_UPDATE)    => replay_update(page, record)
      (RM_HEAP_ID, XLOG_HEAP_HOT_UPDATE)=> replay_hot_update(page, record)
      (RM_HEAP_ID, XLOG_HEAP_LOCK)      => replay_lock(page, record)
      (RM_HEAP_ID, XLOG_HEAP_CONFIRM)   => replay_confirm(page, record)
      (RM_HEAP_ID, XLOG_HEAP_INPLACE)   => replay_inplace(page, record)
      (RM_HEAP2_ID, XLOG_HEAP2_MULTI_INSERT) => replay_multi_insert(page, record)
      (RM_HEAP2_ID, XLOG_HEAP2_PRUNE_*)      => replay_prune_freeze(page, record)
      (RM_HEAP2_ID, XLOG_HEAP2_VISIBLE)      => replay_visible(page, record)
      (RM_HEAP2_ID, XLOG_HEAP2_LOCK_UPDATED) => replay_lock_updated(page, record)

  // After applying, update page LSN
  PageSetLSN(page, record.lsn)
```

### 14.4 LSN Check Optimization

Before applying any record, check:
```rust
if page_lsn >= record_lsn {
    // Page is already at or past this record -- skip
    continue;
}
```

This is the standard "LSN interlock" used by PostgreSQL's own recovery.

### 14.5 Performance Considerations

**The fundamental problem**: WAL is a sequential log with no index. To find
records for a specific page, you must scan every record in the range. For
a busy database, this can mean scanning gigabytes of WAL.

**Mitigation strategies**:

1. **Pre-filter by resource manager**: Only decode RM_HEAP_ID and RM_HEAP2_ID
   records for heap page replay. Skip all other rmgr records by jumping
   `xl_tot_len` bytes ahead.

2. **Block reference quick-check**: Parse just the block headers (cheap)
   before decoding the full record payload.

3. **FPI short-circuit**: If you find an FPI for your target page, discard
   all previously collected records -- the FPI replaces them entirely.

4. **Batch scanning**: When reading multiple pages from the same table,
   scan WAL once and collect records for all target pages simultaneously.

5. **Checkpoint awareness**: Pages flushed after a checkpoint are guaranteed
   to have LSN >= checkpoint's redo LSN. A page's on-disk LSN tells you
   exactly where to start scanning.

---

## 15. Version Differences (PG14 through PG master)

### 15.0 Breaking Changes Summary — What Actually Affects pg_arrow's Parser

Most of the WAL format is stable across versions. Only two changes are truly breaking:

**Breaking Change 1: FPI Compression (PG15+)**

PG14 only supports `pglz` for FPI compression. PG15 added LZ4 and ZSTD:

```
PG14:     bimg_info & 0x04  → pglz only
PG15+:    bimg_info & 0x04  → pglz
          bimg_info & 0x08  → LZ4       ← NEW
          bimg_info & 0x10  → ZSTD      ← NEW
```

Impact: If `wal_compression = lz4` or `zstd` (not default), FPI decompression fails
without LZ4/ZSTD support. With `wal_compression = off` or `pglz`, no impact.

**Breaking Change 2: Vacuum/Freeze Record Unification (PG17+)**

PG14-16 have separate records for vacuum cleanup and tuple freezing. PG17+ unified them
into a single PRUNE record — **and shifted opcodes for other HEAP2 records**:

```
RM_HEAP2_ID opcodes — side-by-side comparison:

  Opcode    PG14-16                       PG17+ (PG18, master)
  ------    -------                       --------------------
  0x00      REWRITE                       REWRITE
  0x10      (unused)                      PRUNE_ON_ACCESS         ← NEW (replaces CLEAN)
  0x20      (unused)                      PRUNE_VACUUM_SCAN       ← NEW (replaces FREEZE_PAGE)
  0x30      CLEAN                         PRUNE_VACUUM_CLEANUP    ← NEW
  0x40      FREEZE_PAGE                   VISIBLE                 ← WAS 0x50 in PG14-16
  0x50      VISIBLE                       MULTI_INSERT            ← WAS 0x60 in PG14-16
  0x60      MULTI_INSERT                  LOCK_UPDATED            ← WAS 0x70 in PG14-16
  0x70      LOCK_UPDATED                  NEW_CID                 ← WAS 0x80 in PG14-16
  0x80      NEW_CID                       (unused)
```

**Every HEAP2 opcode from VISIBLE onwards shifted down by 0x10 in PG17+.** A parser
that hardcodes PG14-16 opcodes will misinterpret every VISIBLE, MULTI_INSERT,
LOCK_UPDATED, and NEW_CID record on PG17+.

**Minor difference within PG17+:**

```
PG17-18:   xl_heap_prune = { uint8 reason, uint8 flags }     (2 separate bytes)
PG master: xl_heap_prune = { uint16 flags }                   (reason merged into flags)
           Added: XLHP_VM_ALL_VISIBLE (1<<8), XLHP_VM_ALL_FROZEN (1<<9)
```

**What's stable across ALL versions (PG14-master):**

| Component | Stable? | Notes |
|-----------|---------|-------|
| XLogRecord header (24B) | Yes | Same layout, same offsets |
| XLogPageHeaderData (24B/40B) | Yes | Same layout |
| Block reference base header (4B) | Yes | Same layout |
| FPI header (5B) | Yes | Same layout (new compression flags, but same struct) |
| RelFileLocator / RelFileNode (12B) | Yes | Same binary layout (names changed PG16) |
| RM_HEAP_ID opcodes (INSERT/DELETE/UPDATE/HOT_UPDATE/LOCK) | Yes | Same values, same payload structs |
| xl_heap_insert (3B) | Yes | Same struct |
| xl_heap_delete (8B) | Yes | Same struct |
| xl_heap_update (14B) | Yes | Same struct |
| xl_heap_lock (8B) | Yes | Same struct |
| LSN arithmetic | Yes | Same formulas |
| CRC-32C | Yes | Same algorithm |
| Continuation records | Yes | Same mechanism |
| Two-phase header/data layout | Yes | Same decoding loop |

**Practical impact for pg_arrow:**

| Parser component | Version-specific? | What to do |
|-----------------|-------------------|------------|
| Record/page header parsing | No | Single code path |
| INSERT/DELETE/UPDATE replay | No | Single code path |
| FPI decompression | Yes (PG15+) | Add LZ4/ZSTD decompressor, dispatch on bimg_info |
| MULTI_INSERT replay | Yes (opcode differs) | Dispatch on version: 0x60 (PG14-16) vs 0x50 (PG17+) |
| VISIBLE replay | Yes (opcode differs) | Dispatch on version: 0x50 (PG14-16) vs 0x40 (PG17+) |
| Vacuum/freeze replay | Yes (completely different) | PG14-16: xl_heap_clean + xl_heap_freeze_page; PG17+: xl_heap_prune with sub-records |
| PRUNE flags parsing | Yes (PG17 vs master) | PG17-18: 2x uint8; master: 1x uint16 |

**Recommendation**: Target PG17/18 first. This avoids the PG14-16 vacuum/freeze structs
entirely and gives you the unified prune/freeze records. Add PG14-16 support later if needed.

### 15.1 PG14 (XLOG_PAGE_MAGIC = 0xD110)

- RelFileNode (spcNode, dbNode, relNode) -- 12 bytes, same layout as RelFileLocator
- No LZ4/ZSTD FPI compression (only pglz)
- Separate XLOG_HEAP2_CLEAN, XLOG_HEAP2_FREEZE_PAGE records (not unified prune)
- xl_heap_clean and xl_heap_freeze_page are separate structures

### 15.2 PG15 (XLOG_PAGE_MAGIC = 0xD113)

- **FPI compression**: Added LZ4 (`BKPIMAGE_COMPRESS_LZ4 = 0x08`) and
  ZSTD (`BKPIMAGE_COMPRESS_ZSTD = 0x10`) via `wal_compression` GUC
- Still uses RelFileNode
- Still has separate XLOG_HEAP2_CLEAN and XLOG_HEAP2_FREEZE_PAGE

### 15.3 PG16 (XLOG_PAGE_MAGIC = 0xD114)

- **RelFileNode renamed to RelFileLocator**: Same binary layout (3 x uint32 = 12 bytes),
  but field names changed: spcNode->spcOid, dbNode->dbOid, relNode->relNumber
- **RelFileNumber**: New typedef (= Oid = uint32) replacing the old relNode
- Still has separate prune/freeze records

### 15.4 PG17 (XLOG_PAGE_MAGIC = 0xD116)

- **Unified pruning/freezing WAL records**: XLOG_HEAP2_CLEAN and
  XLOG_HEAP2_FREEZE_PAGE replaced by XLOG_HEAP2_PRUNE_ON_ACCESS,
  XLOG_HEAP2_PRUNE_VACUUM_SCAN, XLOG_HEAP2_PRUNE_VACUUM_CLEANUP
- xl_heap_prune is the new unified structure with sub-records
- The xl_heap_prune uses `uint8 reason` + `uint8 flags` on PG17

### 15.5 PG18 (XLOG_PAGE_MAGIC = 0xD118)

- xl_heap_prune still uses `uint8 reason` + `uint8 flags` (2 separate bytes)
- SizeOfHeapPrune = 2 (same total, but split differently from master)
- RecoveryTargetAction enum still exists in xlog_internal.h

### 15.6 PG master/latest (XLOG_PAGE_MAGIC = 0xD11A)

- xl_heap_prune flags field changed to `uint16` (merged reason+flags into one)
- Added `XLHP_VM_ALL_VISIBLE` (1<<8) and `XLHP_VM_ALL_FROZEN` (1<<9) flags
- SizeOfHeapPrune = 2 (sizeof(uint16))
- RecoveryTargetAction enum removed from xlog_internal.h
- heap_xlog_deserialize_prune_and_freeze takes `uint16 flags` instead of `uint8`

### 15.7 Handling Multiple Versions in Rust

```rust
enum WalVersion {
    Pg14,  // 0xD110
    Pg15,  // 0xD113
    Pg16,  // 0xD114
    Pg17,  // 0xD116
    Pg18,  // 0xD118
    PgDev, // 0xD11A
}

impl WalVersion {
    fn from_page_magic(magic: u16) -> Result<Self, Error> {
        match magic {
            0xD110 => Ok(Self::Pg14),
            0xD113 => Ok(Self::Pg15),
            0xD114 => Ok(Self::Pg16),
            0xD116 => Ok(Self::Pg17),
            0xD118 => Ok(Self::Pg18),
            0xD11A => Ok(Self::PgDev),
            _ => Err(Error::UnsupportedWalVersion(magic)),
        }
    }

    fn has_lz4_zstd_compression(&self) -> bool {
        matches!(self, Self::Pg15 | Self::Pg16 | Self::Pg17 | Self::Pg18 | Self::PgDev)
    }

    fn uses_relfilelocator(&self) -> bool {
        // PG16+ uses RelFileLocator naming, but the binary layout is the same
        // for all versions. This matters only for code clarity, not parsing.
        matches!(self, Self::Pg16 | Self::Pg17 | Self::Pg18 | Self::PgDev)
    }

    fn has_unified_prune_freeze(&self) -> bool {
        matches!(self, Self::Pg17 | Self::Pg18 | Self::PgDev)
    }

    fn prune_flags_is_u16(&self) -> bool {
        matches!(self, Self::PgDev)
    }
}
```

---

## 16. Complete Rust Struct Definitions

These are the structs you need to parse WAL. All use `repr(C, packed)` because
WAL data is NOT aligned internally.

```rust
use std::fmt;

// ============================================================
// Core types
// ============================================================

pub type XLogRecPtr = u64;     // LSN
pub type TransactionId = u32;
pub type TimeLineID = u32;
pub type BlockNumber = u32;
pub type Oid = u32;
pub type RelFileNumber = Oid;
pub type OffsetNumber = u16;
pub type CommandId = u32;
pub type RepOriginId = u16;
pub type RmgrId = u8;

pub const INVALID_XLOG_REC_PTR: XLogRecPtr = 0;
pub const INVALID_TRANSACTION_ID: TransactionId = 0;
pub const FIRST_COMMAND_ID: CommandId = 0;
pub const INVALID_BLOCK_NUMBER: BlockNumber = 0xFFFFFFFF;
pub const INVALID_OFFSET_NUMBER: OffsetNumber = 0;

// WAL constants
pub const XLOG_BLCKSZ: usize = 8192;
pub const BLCKSZ: usize = 8192;
pub const DEFAULT_WAL_SEG_SIZE: usize = 16 * 1024 * 1024; // 16 MB

// MAXALIGN: 8 bytes on 64-bit
pub const MAXALIGN_SIZE: usize = 8;

pub fn maxalign(len: usize) -> usize {
    (len + MAXALIGN_SIZE - 1) & !(MAXALIGN_SIZE - 1)
}

// ============================================================
// Page headers
// ============================================================

pub const SIZE_OF_XLOG_SHORT_PHD: usize = 24; // MAXALIGN(20)
pub const SIZE_OF_XLOG_LONG_PHD: usize = 40;  // MAXALIGN(36)

#[derive(Debug, Clone)]
pub struct XLogPageHeader {
    pub xlp_magic: u16,
    pub xlp_info: u16,
    pub xlp_tli: TimeLineID,
    pub xlp_pageaddr: XLogRecPtr,
    pub xlp_rem_len: u32,
}

#[derive(Debug, Clone)]
pub struct XLogLongPageHeader {
    pub std: XLogPageHeader,
    pub xlp_sysid: u64,
    pub xlp_seg_size: u32,
    pub xlp_xlog_blcksz: u32,
}

// Page info flags
pub const XLP_FIRST_IS_CONTRECORD: u16 = 0x0001;
pub const XLP_LONG_HEADER: u16 = 0x0002;
pub const XLP_BKP_REMOVABLE: u16 = 0x0004;
pub const XLP_FIRST_IS_OVERWRITE_CONTRECORD: u16 = 0x0008;

impl XLogPageHeader {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            xlp_magic: u16::from_ne_bytes([buf[0], buf[1]]),
            xlp_info: u16::from_ne_bytes([buf[2], buf[3]]),
            xlp_tli: u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]),
            xlp_pageaddr: u64::from_ne_bytes(buf[8..16].try_into().unwrap()),
            xlp_rem_len: u32::from_ne_bytes(buf[16..20].try_into().unwrap()),
        }
    }

    pub fn header_size(&self) -> usize {
        if self.xlp_info & XLP_LONG_HEADER != 0 {
            SIZE_OF_XLOG_LONG_PHD
        } else {
            SIZE_OF_XLOG_SHORT_PHD
        }
    }

    pub fn is_contrecord(&self) -> bool {
        self.xlp_info & XLP_FIRST_IS_CONTRECORD != 0
    }
}

// ============================================================
// WAL Record header
// ============================================================

pub const SIZE_OF_XLOG_RECORD: usize = 24;

#[derive(Debug, Clone)]
pub struct XLogRecord {
    pub xl_tot_len: u32,
    pub xl_xid: TransactionId,
    pub xl_prev: XLogRecPtr,
    pub xl_info: u8,
    pub xl_rmid: RmgrId,
    // 2 bytes padding
    pub xl_crc: u32,
}

impl XLogRecord {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            xl_tot_len: u32::from_ne_bytes(buf[0..4].try_into().unwrap()),
            xl_xid: u32::from_ne_bytes(buf[4..8].try_into().unwrap()),
            xl_prev: u64::from_ne_bytes(buf[8..16].try_into().unwrap()),
            xl_info: buf[16],
            xl_rmid: buf[17],
            // buf[18..20] = padding
            xl_crc: u32::from_ne_bytes(buf[20..24].try_into().unwrap()),
        }
    }

    /// Get the rmgr-specific opcode (high 4 bits of xl_info)
    pub fn rmgr_info(&self) -> u8 {
        self.xl_info & 0xF0
    }
}

// xl_info flags
pub const XLR_INFO_MASK: u8 = 0x0F;
pub const XLR_RMGR_INFO_MASK: u8 = 0xF0;
pub const XLR_SPECIAL_REL_UPDATE: u8 = 0x01;
pub const XLR_CHECK_CONSISTENCY: u8 = 0x02;

// ============================================================
// Block reference header
// ============================================================

pub const SIZE_OF_XLOG_RECORD_BLOCK_HEADER: usize = 4;
pub const SIZE_OF_XLOG_RECORD_BLOCK_IMAGE_HEADER: usize = 5;
pub const SIZE_OF_XLOG_RECORD_BLOCK_COMPRESS_HEADER: usize = 2;
pub const SIZE_OF_REL_FILE_LOCATOR: usize = 12;

// fork_flags
pub const BKPBLOCK_FORK_MASK: u8 = 0x0F;
pub const BKPBLOCK_FLAG_MASK: u8 = 0xF0;
pub const BKPBLOCK_HAS_IMAGE: u8 = 0x10;
pub const BKPBLOCK_HAS_DATA: u8 = 0x20;
pub const BKPBLOCK_WILL_INIT: u8 = 0x40;
pub const BKPBLOCK_SAME_REL: u8 = 0x80;

// bimg_info
pub const BKPIMAGE_HAS_HOLE: u8 = 0x01;
pub const BKPIMAGE_APPLY: u8 = 0x02;
pub const BKPIMAGE_COMPRESS_PGLZ: u8 = 0x04;
pub const BKPIMAGE_COMPRESS_LZ4: u8 = 0x08;
pub const BKPIMAGE_COMPRESS_ZSTD: u8 = 0x10;

pub fn bkpimage_is_compressed(bimg_info: u8) -> bool {
    (bimg_info & (BKPIMAGE_COMPRESS_PGLZ | BKPIMAGE_COMPRESS_LZ4 | BKPIMAGE_COMPRESS_ZSTD)) != 0
}

// Block IDs
pub const XLR_MAX_BLOCK_ID: u8 = 32;
pub const XLR_BLOCK_ID_DATA_SHORT: u8 = 255;
pub const XLR_BLOCK_ID_DATA_LONG: u8 = 254;
pub const XLR_BLOCK_ID_ORIGIN: u8 = 253;
pub const XLR_BLOCK_ID_TOPLEVEL_XID: u8 = 252;

#[derive(Debug, Clone)]
pub struct RelFileLocator {
    pub spc_oid: Oid,
    pub db_oid: Oid,
    pub rel_number: RelFileNumber,
}

impl RelFileLocator {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            spc_oid: u32::from_ne_bytes(buf[0..4].try_into().unwrap()),
            db_oid: u32::from_ne_bytes(buf[4..8].try_into().unwrap()),
            rel_number: u32::from_ne_bytes(buf[8..12].try_into().unwrap()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedBlockRef {
    pub block_id: u8,
    pub rlocator: RelFileLocator,
    pub forknum: u8,
    pub blkno: BlockNumber,
    pub fork_flags: u8,

    // FPI info
    pub has_image: bool,
    pub apply_image: bool,
    pub bimg_len: u16,
    pub hole_offset: u16,
    pub hole_length: u16,
    pub bimg_info: u8,

    // Block data
    pub has_data: bool,
    pub data_len: u16,

    // Byte ranges into the reassembled record payload
    pub image_offset: usize,  // offset into data portion where FPI bytes start
    pub data_offset: usize,   // offset into data portion where block data starts
}

// ============================================================
// Resource Manager IDs
// ============================================================

pub const RM_XLOG_ID: RmgrId = 0;
pub const RM_XACT_ID: RmgrId = 1;
pub const RM_SMGR_ID: RmgrId = 2;
pub const RM_CLOG_ID: RmgrId = 3;
pub const RM_DBASE_ID: RmgrId = 4;
pub const RM_TBLSPC_ID: RmgrId = 5;
pub const RM_MULTIXACT_ID: RmgrId = 6;
pub const RM_RELMAP_ID: RmgrId = 7;
pub const RM_STANDBY_ID: RmgrId = 8;
pub const RM_HEAP2_ID: RmgrId = 9;
pub const RM_HEAP_ID: RmgrId = 10;
pub const RM_BTREE_ID: RmgrId = 11;

// ============================================================
// Heap WAL opcodes (RM_HEAP_ID)
// ============================================================

pub const XLOG_HEAP_INSERT: u8 = 0x00;
pub const XLOG_HEAP_DELETE: u8 = 0x10;
pub const XLOG_HEAP_UPDATE: u8 = 0x20;
pub const XLOG_HEAP_TRUNCATE: u8 = 0x30;
pub const XLOG_HEAP_HOT_UPDATE: u8 = 0x40;
pub const XLOG_HEAP_CONFIRM: u8 = 0x50;
pub const XLOG_HEAP_LOCK: u8 = 0x60;
pub const XLOG_HEAP_INPLACE: u8 = 0x70;

pub const XLOG_HEAP_OPMASK: u8 = 0x70;
pub const XLOG_HEAP_INIT_PAGE: u8 = 0x80;

// ============================================================
// Heap2 WAL opcodes (RM_HEAP2_ID)
// ============================================================

pub const XLOG_HEAP2_REWRITE: u8 = 0x00;
pub const XLOG_HEAP2_PRUNE_ON_ACCESS: u8 = 0x10;
pub const XLOG_HEAP2_PRUNE_VACUUM_SCAN: u8 = 0x20;
pub const XLOG_HEAP2_PRUNE_VACUUM_CLEANUP: u8 = 0x30;
pub const XLOG_HEAP2_VISIBLE: u8 = 0x40;
pub const XLOG_HEAP2_MULTI_INSERT: u8 = 0x50;
pub const XLOG_HEAP2_LOCK_UPDATED: u8 = 0x60;
pub const XLOG_HEAP2_NEW_CID: u8 = 0x70;

// ============================================================
// Heap record payload structs
// ============================================================

pub const SIZE_OF_HEAP_HEADER: usize = 5;
pub const SIZE_OF_HEAP_INSERT: usize = 3;
pub const SIZE_OF_HEAP_DELETE: usize = 8;
pub const SIZE_OF_HEAP_UPDATE: usize = 14;
pub const SIZE_OF_HEAP_MULTI_INSERT: usize = 3; // before offsets array
pub const SIZE_OF_MULTI_INSERT_TUPLE: usize = 7;

#[derive(Debug, Clone)]
pub struct XlHeapHeader {
    pub t_infomask2: u16,
    pub t_infomask: u16,
    pub t_hoff: u8,
}

impl XlHeapHeader {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            t_infomask2: u16::from_ne_bytes(buf[0..2].try_into().unwrap()),
            t_infomask: u16::from_ne_bytes(buf[2..4].try_into().unwrap()),
            t_hoff: buf[4],
        }
    }
}

#[derive(Debug, Clone)]
pub struct XlHeapInsert {
    pub offnum: OffsetNumber,
    pub flags: u8,
}

impl XlHeapInsert {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            offnum: u16::from_ne_bytes(buf[0..2].try_into().unwrap()),
            flags: buf[2],
        }
    }
}

#[derive(Debug, Clone)]
pub struct XlHeapDelete {
    pub xmax: TransactionId,
    pub offnum: OffsetNumber,
    pub infobits_set: u8,
    pub flags: u8,
}

impl XlHeapDelete {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            xmax: u32::from_ne_bytes(buf[0..4].try_into().unwrap()),
            offnum: u16::from_ne_bytes(buf[4..6].try_into().unwrap()),
            infobits_set: buf[6],
            flags: buf[7],
        }
    }
}

#[derive(Debug, Clone)]
pub struct XlHeapUpdate {
    pub old_xmax: TransactionId,
    pub old_offnum: OffsetNumber,
    pub old_infobits_set: u8,
    pub flags: u8,
    pub new_xmax: TransactionId,
    pub new_offnum: OffsetNumber,
}

impl XlHeapUpdate {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            old_xmax: u32::from_ne_bytes(buf[0..4].try_into().unwrap()),
            old_offnum: u16::from_ne_bytes(buf[4..6].try_into().unwrap()),
            old_infobits_set: buf[6],
            flags: buf[7],
            new_xmax: u32::from_ne_bytes(buf[8..12].try_into().unwrap()),
            new_offnum: u16::from_ne_bytes(buf[12..14].try_into().unwrap()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XlHeapMultiInsert {
    pub flags: u8,
    pub ntuples: u16,
    // offsets follow in the main data (unless INIT_PAGE)
}

impl XlHeapMultiInsert {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            flags: buf[0],
            ntuples: u16::from_ne_bytes(buf[1..3].try_into().unwrap()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XlMultiInsertTuple {
    pub datalen: u16,
    pub t_infomask2: u16,
    pub t_infomask: u16,
    pub t_hoff: u8,
}

impl XlMultiInsertTuple {
    pub fn parse(buf: &[u8]) -> Self {
        Self {
            datalen: u16::from_ne_bytes(buf[0..2].try_into().unwrap()),
            t_infomask2: u16::from_ne_bytes(buf[2..4].try_into().unwrap()),
            t_infomask: u16::from_ne_bytes(buf[4..6].try_into().unwrap()),
            t_hoff: buf[6],
        }
    }
}

// ============================================================
// Insert/Delete/Update flag constants
// ============================================================

// xl_heap_insert flags
pub const XLH_INSERT_ALL_VISIBLE_CLEARED: u8 = 1 << 0;
pub const XLH_INSERT_LAST_IN_MULTI: u8 = 1 << 1;
pub const XLH_INSERT_IS_SPECULATIVE: u8 = 1 << 2;
pub const XLH_INSERT_CONTAINS_NEW_TUPLE: u8 = 1 << 3;
pub const XLH_INSERT_ON_TOAST_RELATION: u8 = 1 << 4;
pub const XLH_INSERT_ALL_FROZEN_SET: u8 = 1 << 5;

// xl_heap_delete flags
pub const XLH_DELETE_ALL_VISIBLE_CLEARED: u8 = 1 << 0;
pub const XLH_DELETE_CONTAINS_OLD_TUPLE: u8 = 1 << 1;
pub const XLH_DELETE_CONTAINS_OLD_KEY: u8 = 1 << 2;
pub const XLH_DELETE_IS_SUPER: u8 = 1 << 3;
pub const XLH_DELETE_IS_PARTITION_MOVE: u8 = 1 << 4;

// xl_heap_update flags
pub const XLH_UPDATE_OLD_ALL_VISIBLE_CLEARED: u8 = 1 << 0;
pub const XLH_UPDATE_NEW_ALL_VISIBLE_CLEARED: u8 = 1 << 1;
pub const XLH_UPDATE_CONTAINS_OLD_TUPLE: u8 = 1 << 2;
pub const XLH_UPDATE_CONTAINS_OLD_KEY: u8 = 1 << 3;
pub const XLH_UPDATE_CONTAINS_NEW_TUPLE: u8 = 1 << 4;
pub const XLH_UPDATE_PREFIX_FROM_OLD: u8 = 1 << 5;
pub const XLH_UPDATE_SUFFIX_FROM_OLD: u8 = 1 << 6;

// infobits_set constants (for xl_heap_delete, xl_heap_update, xl_heap_lock)
pub const XLHL_XMAX_IS_MULTI: u8 = 0x01;
pub const XLHL_XMAX_LOCK_ONLY: u8 = 0x02;
pub const XLHL_XMAX_EXCL_LOCK: u8 = 0x04;
pub const XLHL_XMAX_KEYSHR_LOCK: u8 = 0x08;
pub const XLHL_KEYS_UPDATED: u8 = 0x10;

// HeapTupleHeader infomask bits (needed for replay)
pub const HEAP_HASNULL: u16 = 0x0001;
pub const HEAP_HASVARWIDTH: u16 = 0x0002;
pub const HEAP_HASEXTERNAL: u16 = 0x0004;
pub const HEAP_XMAX_KEYSHR_LOCK: u16 = 0x0010;
pub const HEAP_COMBOCID: u16 = 0x0020;
pub const HEAP_XMAX_EXCL_LOCK: u16 = 0x0040;
pub const HEAP_XMAX_LOCK_ONLY: u16 = 0x0080;
pub const HEAP_XMAX_SHR_LOCK: u16 = HEAP_XMAX_EXCL_LOCK | HEAP_XMAX_KEYSHR_LOCK;
pub const HEAP_XMIN_COMMITTED: u16 = 0x0100;
pub const HEAP_XMIN_INVALID: u16 = 0x0200;
pub const HEAP_XMAX_COMMITTED: u16 = 0x0400;
pub const HEAP_XMAX_INVALID: u16 = 0x0800;
pub const HEAP_XMAX_IS_MULTI: u16 = 0x1000;
pub const HEAP_UPDATED: u16 = 0x2000;
pub const HEAP_MOVED_OFF: u16 = 0x4000;
pub const HEAP_MOVED_IN: u16 = 0x8000;
pub const HEAP_MOVED: u16 = HEAP_MOVED_OFF | HEAP_MOVED_IN;
pub const HEAP_XMAX_BITS: u16 = HEAP_XMAX_COMMITTED | HEAP_XMAX_INVALID |
    HEAP_XMAX_IS_MULTI | HEAP_XMAX_LOCK_ONLY | HEAP_XMAX_KEYSHR_LOCK | HEAP_XMAX_EXCL_LOCK;
pub const HEAP_KEYS_UPDATED: u16 = 0x2000; // in t_infomask2

// SizeofHeapTupleHeader = 23 bytes (offsetof t_bits)
pub const SIZE_OF_HEAP_TUPLE_HEADER: usize = 23;
```

---

## 17. ForkNumber Reference

```
MAIN_FORKNUM          = 0  // Heap/index data
FSM_FORKNUM           = 1  // Free space map
VISIBILITYMAP_FORKNUM = 2  // Visibility map
INIT_FORKNUM          = 3  // Init fork (for unlogged tables)
```

For pg_arrow WAL replay, you care about `MAIN_FORKNUM = 0` (the heap data).
You may also need to handle `VISIBILITYMAP_FORKNUM = 2` if you want to track
all-visible/all-frozen status.

---

## 18. Key Source Files Reference

| File | What It Defines |
|------|----------------|
| `src/include/access/xlogdefs.h` | XLogRecPtr, TimeLineID, XLogSegNo |
| `src/include/access/xlogrecord.h` | XLogRecord, XLogRecordBlockHeader, block IDs |
| `src/include/access/xlog_internal.h` | XLogPageHeaderData, XLogLongPageHeaderData, file naming |
| `src/include/access/xlogreader.h` | DecodedBkpBlock, DecodedXLogRecord, reader API |
| `src/include/access/heapam_xlog.h` | All heap WAL record types and flags |
| `src/include/access/rmgr.h` | RmgrId enum definition |
| `src/include/access/rmgrlist.h` | Resource manager ID assignments |
| `src/include/storage/relfilelocator.h` | RelFileLocator (PG16+) / RelFileNode |
| `src/include/storage/block.h` | BlockNumber, BlockIdData |
| `src/include/common/relpath.h` | RelFileNumber, ForkNumber |
| `src/backend/access/transam/xlogreader.c` | DecodeXLogRecord, RestoreBlockImage |
| `src/backend/access/heap/heapam_xlog.c` | heap_redo, heap2_redo (all replay functions) |

---

## 19. Existing Implementations and References

### 19.1 pg_walinspect (PostgreSQL contrib)

PostgreSQL's own WAL inspection extension. Uses the standard XLogReader
infrastructure. Good reference for what fields are available after decoding.

### 19.2 pg_waldump (PostgreSQL tool)

Command-line tool that decodes and prints WAL records. Source is in
`src/bin/pg_waldump/`. This is the simplest standalone WAL reader and the
best reference for a from-scratch implementation.

**Source**: `src/bin/pg_waldump/pg_waldump.c`

### 19.3 wal2json / pgoutput

Logical decoding plugins. These work at a higher level (after PostgreSQL has
already decoded the WAL). Not directly useful for physical WAL parsing, but
the concept of filtering records by relation is relevant.

### 19.4 neon (Zenith/Neon)

The Neon project (Rust-based PostgreSQL storage) has a WAL parser in Rust.
Their crate `postgres_ffi` provides Rust bindings for PostgreSQL WAL structures.

**Key insight from Neon**: They define the WAL structs version-specifically
(different modules for PG14, PG15, PG16, PG17) and dispatch based on the
page magic. This is the recommended approach for multi-version support.

**Repository**: https://github.com/neondatabase/neon
**Relevant code**: `libs/postgres_ffi/src/` (WAL struct definitions per version)

### 19.5 Key Papers

- P. Helland and D. Campbell, "Building on Quicksand," CIDR 2009.
  (Foundational paper on WAL and recovery in database systems.)

- M. Stonebraker et al., "ARIES: A Transaction Recovery Method Supporting
  Fine-Granularity Locking and Partial Rollbacks Using Write-Ahead Logging,"
  ACM TODS 1992. (The classic WAL/recovery algorithm that PostgreSQL's
  approach is inspired by, though PG does not implement full ARIES.)

---

## 20. Decision: Implementation Strategy for pg_arrow

### Recommended Approach

1. **Start with PG17/18 support only** -- The unified prune/freeze records
   simplify things. Add PG15/16 support later if needed.

2. **Implement the minimal set of record types**:
   - XLOG_HEAP_INSERT (most common for growing tables)
   - XLOG_HEAP_DELETE
   - XLOG_HEAP_UPDATE / HOT_UPDATE
   - XLOG_HEAP2_MULTI_INSERT (critical for COPY operations)
   - FPI restoration (handles all other cases implicitly)
   - Skip: LOCK, CONFIRM, INPLACE, TRUNCATE (not needed for read consistency)

3. **Use FPIs aggressively**: When an FPI is found for a target page, use it
   directly and discard any incremental records before it. FPIs are complete
   page snapshots and bypass all the complexity of individual tuple replay.

4. **For the WAL scan**: Initially do a linear scan. Later optimize with:
   - An in-memory index of WAL records by (relfilelocator, blkno)
   - Pre-scanning WAL in background threads
   - Caching the scan results for recently-accessed LSN ranges

5. **Validate with pg_waldump**: Use `pg_waldump` to verify your parser produces
   the same record breakdown for the same WAL range.

### Future Considerations

- **Streaming WAL**: For the sidecar mode, implement WAL streaming via the
  replication protocol to get WAL records in near-real-time instead of reading
  files.
- **Checkpoint tracking**: Monitor checkpoint records to know which pages are
  guaranteed flushed and don't need WAL replay.
- **TOAST detoasting**: WAL for TOAST tables uses the same format but you need
  to handle the TOAST relation's relfilenode separately.

---

## Changelog

- 2026-02-11: Initial comprehensive WAL format reference covering PG14-master
