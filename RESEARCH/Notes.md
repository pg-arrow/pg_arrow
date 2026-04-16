# Vectored / Scatter-Gather Read Techniques in Rust

Notes on reading from multiple offsets in a file and processing the data.

## 1. `pread` via `FileExt::read_exact_at`

One syscall per offset. Simple, portable, no unsafe.

```rust
use std::os::unix::fs::FileExt;
use std::fs::File;

let file = File::open("base/16384/16385")?;

let mut header_buf = [0u8; 24];
let mut tuple_buf = [0u8; 256];

file.read_exact_at(&mut header_buf, 0)?;       // offset 0
file.read_exact_at(&mut tuple_buf, 8192)?;      // offset 8192
```

**Tradeoff**: Simple but 1 syscall per read. Fine for a small number of reads.

## 2. `preadv` with `IoSliceMut` — Scatter into multiple buffers from one offset

Reads contiguous data into disjoint buffers in a single syscall.

```rust
use std::io::IoSliceMut;
use std::os::fd::AsRawFd;

let mut hdr = [0u8; 24];
let mut body = [0u8; 8168];
let mut bufs = [
    IoSliceMut::new(&mut hdr),
    IoSliceMut::new(&mut body),
];

// Kernel fills hdr first, then body — one syscall, contiguous read
nix::sys::uio::preadv(file.as_raw_fd(), &mut bufs, 0)?;
```

**Tradeoff**: One syscall but reads from a *contiguous* region. Useful when you want the
data split across different buffers (e.g., header vs body) but not for non-contiguous offsets.

## 3. `mmap` + Slice Multiple Offsets (zero-copy)

Best for random access across many offsets — no syscalls after initial `mmap`.

```rust
use memmap2::Mmap;

let file = File::open("base/16384/16385")?;
let mmap = unsafe { Mmap::map(&file)? };

// "Read" from arbitrary offsets — just pointer arithmetic, no syscalls
let page0_header = &mmap[0..24];
let page1_header = &mmap[8192..8216];
let page5_tuples = &mmap[5 * 8192 + 24..5 * 8192 + 100];

// Compute on all of them
let results: Vec<_> = offsets
    .iter()
    .map(|&off| parse_page_header(&mmap[off..off + 24]))
    .collect();
```

**Tradeoff**: Zero-copy, zero syscalls on the read path. The OS page cache handles
prefetching. Risk of SIGBUS if the file is truncated while mapped. Best for read-only
workloads with many random accesses (exactly the pg_arrow use case).

## 4. `io_uring` — Batched async reads at arbitrary offsets (Linux only)

Submit multiple reads at different offsets as a single batch. One syscall for N reads.

```rust
use io_uring::{IoUring, opcode, types};

let mut ring = IoUring::new(64)?;
let fd = types::Fd(file.as_raw_fd());

let offsets = [0, 8192, 16384, 24576];
let mut buffers: Vec<Vec<u8>> = offsets.iter().map(|_| vec![0u8; 8192]).collect();

for (i, &offset) in offsets.iter().enumerate() {
    let entry = opcode::Read::new(fd, buffers[i].as_mut_ptr(), 8192)
        .offset(offset as u64)
        .build()
        .user_data(i as u64);
    unsafe { ring.submission().push(&entry)?; }
}

ring.submit_and_wait(offsets.len())?;  // ONE syscall for all reads

for cqe in ring.completion() {
    let idx = cqe.user_data() as usize;
    let header = parse_page_header(&buffers[idx][..24]);
}
```

**Tradeoff**: Highest throughput for batched random I/O. Linux-only. More complex setup.
Best when you need to control I/O scheduling or read from multiple segment files concurrently.

## 5. Gather → Process Pattern

Common pattern for slicing data from multiple offsets and computing over them:

```rust
struct PageSlice<'a> {
    page_num: u32,
    header: &'a [u8],
    tuples: &'a [u8],
}

fn gather_and_compute(mmap: &Mmap, page_offsets: &[usize]) -> Vec<ArrowBatch> {
    // Gather: slice out the regions you need
    let slices: Vec<PageSlice> = page_offsets
        .iter()
        .map(|&off| {
            let hdr = &mmap[off..off + 24];
            let pd_lower = u16::from_le_bytes([hdr[12], hdr[13]]) as usize;
            let pd_upper = u16::from_le_bytes([hdr[14], hdr[15]]) as usize;
            PageSlice {
                page_num: (off / 8192) as u32,
                header: hdr,
                tuples: &mmap[off + pd_upper..off + 8192],
            }
        })
        .collect();

    // Compute: process each slice (parallelizable with rayon)
    slices.iter().map(|s| parse_tuples_to_arrow(s)).collect()
}
```

## Comparison Table

| Method            | Syscalls     | Random access | Zero-copy | Best for                      |
|-------------------|--------------|---------------|-----------|-------------------------------|
| `read_at` (pread) | 1 per read   | Yes           | No        | Few reads                     |
| `preadv`          | 1 per call   | No (contig.)  | No        | Sequential scatter            |
| `mmap` + slicing  | 0 (after map)| Yes           | Yes       | Many random reads             |
| `io_uring`        | 1 per batch  | Yes           | No        | High-throughput batched I/O   |

## Relevance to pg_arrow

- **Primary choice**: `mmap` + slicing — we do random access across pages, need zero-copy
  for Arrow buffer construction, and the OS page cache handles prefetching.
- **Future optimization**: `io_uring` for reading specific pages from multiple segment files
  concurrently, or for prefetching pages ahead of the parser.
- **Fallback**: `pread` for portability (macOS, where `io_uring` is unavailable).
