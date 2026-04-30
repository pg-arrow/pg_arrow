use std::fs::File;
use std::io::prelude::*;
use std::io::{self, BufReader, Bytes, Read, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};

use crate::file::error::{PgError, Result};
use crate::file::page::PAGES_PER_SEGMENT;
use crate::file::*;

const PAGE_SIZE: usize = 8192;

pub type RawPage = &'static [u8; PAGE_SIZE];
pub type Oid = usize;

/// A trait for reading byte chunks from a data source by offset and length.
///
/// Implementations provide random-access reads that return a [`Read`] handle
/// over the requested byte range. This allows callers to stream data without
/// loading entire files into memory.
pub trait ChunkReader: Send + Sync {
    /// The concrete [`Read`] type returned by [`read_chunk`].
    type T: Read + Send;

    /// Returns a reader over `length` bytes starting at `offset`.
    fn read_chunk(&self, offset: u64, length: u64) -> io::Result<Bytes<&[u8]>>;

    fn get_reader(&self, pos: usize) -> Result<Self::T>;

    /// Bulk-read `count` consecutive pages starting at `start_page` into a
    /// single `bytes::Bytes` buffer. Returns `(buffer, pages_actually_read)`.
    fn read_pages_bulk(&self, start_page: usize, count: usize) -> Result<(bytes::Bytes, usize)>;
}

pub struct TableFileReader {
    pub relation_id: Oid,
    pub db_id: Oid,
    /// Cached file descriptor for the most recently accessed segment.
    /// Avoids reopening the same segment file on every `read_pages_bulk` call.
    segment_cache: Mutex<Option<(usize, File)>>,
}

impl TableFileReader {
    pub fn new(db_id: Oid, relation_id: Oid) -> Self {
        Self {
            relation_id,
            db_id,
            segment_cache: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for TableFileReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableFileReader")
            .field("relation_id", &self.relation_id)
            .field("db_id", &self.db_id)
            .finish()
    }
}

impl Default for TableFileReader {
    fn default() -> Self {
        Self {
            relation_id: 0,
            db_id: 0,
            segment_cache: Mutex::new(None),
        }
    }
}

impl TableFileReader {
    pub fn get_page_reader(self) -> Result<PageReader<TableFileReader>> {
        Ok(PageReader {
            segment_chunk_reader: Arc::new(self),
            reader_instance: None,
        })
    }

    fn get_page_file_and_offset(&self, page_index: usize) -> Result<(PathBuf, usize)> {
        let segment_num = page_index / PAGES_PER_SEGMENT;
        let page_in_segment = page_index % PAGES_PER_SEGMENT;

        let data_dir = crate::file::get_data_dir().unwrap();
        let table_file = PathBuf::from(format!(
            "{}/base/{}/{}",
            data_dir, self.db_id, self.relation_id
        ));

        let segment_path = if segment_num == 0 {
            table_file
        } else {
            PathBuf::from(format!("{}.{}", table_file.display(), segment_num)) // base/16384/24601.1
        };

        Ok((segment_path, page_in_segment * PAGE_BUFFER_SIZE))
    }
}

impl ChunkReader for TableFileReader {
    type T = BufReader<File>;

    fn read_chunk(&self, offset: u64, length: u64) -> io::Result<Bytes<&[u8]>> {
        todo!("read_chunk: seek to offset {offset}, read {length} bytes")
    }

    fn get_reader(&self, pos: usize) -> Result<Self::T> {
        let path = self.get_page_file_and_offset(pos / PAGE_BUFFER_SIZE)?;

        log::info!("Reading from segment file {}", path.0.display());
        let mut table_file = if let Ok(fd) = File::open(path.0) {
            fd
        } else {
            return Err(PgError::Generic);
        };
        table_file
            .seek(SeekFrom::Start(path.1 as u64))
            .map_err(|_| PgError::Generic)?;

        Ok(BufReader::new(table_file))
    }

    fn read_pages_bulk(&self, start_page: usize, count: usize) -> Result<(bytes::Bytes, usize)> {
        if count == 0 {
            return Ok((bytes::Bytes::new(), 0));
        }

        let total_bytes = count * PAGE_BUFFER_SIZE;
        let mut buf = vec![0u8; total_bytes];
        let mut buf_offset = 0usize;
        let mut page_idx = start_page;
        let end_page = start_page + count;

        let mut cache = self.segment_cache.lock().unwrap();

        while page_idx < end_page {
            let segment_num = page_idx / PAGES_PER_SEGMENT;
            let page_in_segment = page_idx % PAGES_PER_SEGMENT;
            let offset_in_segment = page_in_segment * PAGE_BUFFER_SIZE;

            let pages_left_in_segment = PAGES_PER_SEGMENT - page_in_segment;
            let pages_to_read = (end_page - page_idx).min(pages_left_in_segment);
            let read_bytes = pages_to_read * PAGE_BUFFER_SIZE;

            // Reuse cached fd if it's for the same segment, otherwise open new.
            let need_open = match &*cache {
                Some((cached_seg, _)) => *cached_seg != segment_num,
                None => true,
            };
            if need_open {
                let (segment_path, _) = self.get_page_file_and_offset(page_idx)?;
                match File::open(&segment_path) {
                    Ok(f) => *cache = Some((segment_num, f)),
                    Err(_) => break, // segment doesn't exist — EOF
                }
            }
            let fd = cache.as_ref().unwrap().1.as_raw_fd();

            // SAFETY: `buf` is valid for `read_bytes` starting at `buf_offset`,
            // and `fd` is an open file descriptor held alive by the cache guard.
            let n = unsafe {
                libc::pread(
                    fd,
                    buf[buf_offset..].as_mut_ptr() as *mut libc::c_void,
                    read_bytes,
                    offset_in_segment as i64,
                )
            };

            if n <= 0 {
                break;
            }

            let bytes_read = n as usize;
            let pages_read = bytes_read / PAGE_BUFFER_SIZE;
            buf_offset += pages_read * PAGE_BUFFER_SIZE;
            page_idx += pages_read;

            if bytes_read < read_bytes {
                break;
            }
        }

        let pages_read = buf_offset / PAGE_BUFFER_SIZE;
        buf.truncate(buf_offset);
        Ok((bytes::Bytes::from(buf), pages_read))
    }
}

#[derive(Debug)]
pub struct PageReader<R: ChunkReader> {
    // relation_id: Oid,
    segment_chunk_reader: Arc<R>,
    reader_instance: Option<Arc<Mutex<R::T>>>,
}

impl<R: ChunkReader> PageReader<R> {
    pub fn get_page_by_index(&mut self, page_index: usize) -> Result<crate::file::HeapPageData> {
        log::debug!("Page number: {:#?}", page_index);
        let mut page_buffer = [0u8; PAGE_BUFFER_SIZE];

        if self.reader_instance.is_none() {
            let reader_chunk_instance = self
                .segment_chunk_reader
                .get_reader(page_index * PAGE_BUFFER_SIZE)
                .unwrap();
            self.reader_instance = Some(Arc::new(Mutex::new(reader_chunk_instance)));
        }

        let mut chunk_reader = self.reader_instance.as_ref().unwrap().lock().unwrap();
        let read_result = chunk_reader.read_exact(&mut page_buffer);
        drop(chunk_reader);

        if read_result.is_err() {
            // Check the next segment file
            if let Ok(reader_chunk_instance) = self
                .segment_chunk_reader
                .get_reader(page_index * PAGE_BUFFER_SIZE)
            {
                self.reader_instance = Some(Arc::new(Mutex::new(reader_chunk_instance)));
                // TODO: check for race condition on parallel call.
                let mut chunk_reader = self.reader_instance.as_ref().unwrap().lock().unwrap();
                let read_result = chunk_reader.read_exact(&mut page_buffer);
                if read_result.is_err() {
                    return Err(PgError::Generic);
                }
            } else {
                return Err(PgError::Generic);
            };
        }

        let page = HeapPageData::parse(page_buffer)?;
        Ok(page)
    }
}

/// Default number of pages to read per batch. Each page is 8KB, so 128 pages
/// = 1MB of page buffers in memory at a time. Tuned for L2/L3 cache residency
/// while giving enough work to saturate parallel conversion threads.
pub const DEFAULT_PAGES_PER_BATCH: usize = 128;

impl<R: ChunkReader> PageReader<R> {
    /// Create a streaming iterator that yields `RecordBatch`es one at a time.
    ///
    /// Internally reads pages in batches of `DEFAULT_PAGES_PER_BATCH`, converts
    /// them in parallel, and yields individual `RecordBatch`es lazily. Page
    /// buffers from one batch are freed before the next is read.
    ///
    /// ```no_run
    /// # use pg_arrow::file::reader::TableFileReader;
    /// # use pg_arrow::types::{PgCatalogRelation, PgClass};
    /// let schema = PgClass::catalog_schema();
    /// let reader = TableFileReader::new(16384, 1259);
    /// let page_reader = reader.get_page_reader().unwrap();
    ///
    /// for batch_result in page_reader.into_batch_stream(&schema, None) {
    ///     let batch = batch_result.unwrap();
    ///     println!("got {} rows", batch.num_rows());
    /// }
    /// ```
    pub fn into_batch_stream(
        self,
        schema: &crate::types::PgSchema,
        projection: Option<&[usize]>,
    ) -> RecordBatchStream<R> {
        RecordBatchStream::new(self, schema.clone(), projection.map(|p| p.to_vec()))
    }

    /// Collect all pages into `RecordBatch`es. Convenience wrapper around
    /// [`into_batch_stream`] for cases where you need all results at once.
    pub fn read_all_to_batches(
        self,
        schema: &crate::types::PgSchema,
        projection: Option<&[usize]>,
    ) -> Result<Vec<arrow::record_batch::RecordBatch>> {
        let mut stream =
            RecordBatchStream::new(self, schema.clone(), projection.map(|p| p.to_vec()));
        let mut all = Vec::new();
        for result in &mut stream {
            all.push(result?);
        }
        Ok(all)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// RecordBatchStream — streaming iterator over RecordBatches
// ────────────────────────────────────────────────────────────────────────────

/// A streaming iterator that yields one `RecordBatch` per heap page.
///
/// Pages are read in configurable batches (default 128 = 1MB), converted to
/// `RecordBatch`es in parallel, then yielded individually. When the internal
/// buffer drains, the next batch of pages is read and converted on demand.
///
/// **Memory profile**: at most `pages_per_batch × 8KB` of page buffers plus
/// the current batch's Arrow arrays. Previous batches' page buffers are freed
/// before the next I/O round.
pub struct RecordBatchStream<R: ChunkReader> {
    page_reader: PageReader<R>,
    schema: crate::types::PgSchema,
    projection: Option<Vec<usize>>,
    pub(crate) pages_per_batch: usize,
    num_threads: usize,

    /// Next page index to read from the file.
    pub(crate) next_page_index: usize,
    /// Exclusive upper bound on page indices to read. `None` means read until EOF.
    pub(crate) max_page_index: Option<usize>,
    /// Buffered RecordBatches from the current page batch, served FIFO.
    pub(crate) buffer: std::collections::VecDeque<arrow::record_batch::RecordBatch>,
    /// True once we've read past the last page.
    pub(crate) exhausted: bool,
}

impl<R: ChunkReader> RecordBatchStream<R> {
    pub(crate) fn new(
        page_reader: PageReader<R>,
        schema: crate::types::PgSchema,
        projection: Option<Vec<usize>>,
    ) -> Self {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            page_reader,
            schema,
            projection,
            pages_per_batch: DEFAULT_PAGES_PER_BATCH,
            num_threads,
            next_page_index: 0,
            max_page_index: None,
            buffer: std::collections::VecDeque::new(),
            exhausted: false,
        }
    }

    /// Set the number of pages to read per I/O batch.
    pub fn with_pages_per_batch(mut self, n: usize) -> Self {
        self.pages_per_batch = if n == 0 { DEFAULT_PAGES_PER_BATCH } else { n };
        self
    }

    /// Restrict this stream to pages `[start, end)`.
    ///
    /// Useful for splitting a table scan across multiple partitions.
    pub fn with_page_range(mut self, start: usize, end: usize) -> Self {
        self.next_page_index = start;
        self.max_page_index = Some(end);
        self
    }

    /// Fill the internal buffer by reading and converting the next batch of pages.
    ///
    /// Reads pages in bulk via `read_pages_bulk` and parses them using
    /// zero-copy `Bytes` slicing.
    pub fn fill_buffer(&mut self) -> Result<()> {
        if self.exhausted {
            return Ok(());
        }

        let want = if let Some(max) = self.max_page_index {
            self.pages_per_batch
                .min(max.saturating_sub(self.next_page_index))
        } else {
            self.pages_per_batch
        };

        if want == 0 {
            self.exhausted = true;
            return Ok(());
        }

        let (bulk_bytes, pages_read) = self
            .page_reader
            .segment_chunk_reader
            .read_pages_bulk(self.next_page_index, want)?;

        if pages_read == 0 {
            self.exhausted = true;
            return Ok(());
        }
        let proj_slice = self.projection.as_deref();

        for i in 0..pages_read {
            let start = i * PAGE_BUFFER_SIZE;
            let end = start + PAGE_BUFFER_SIZE;
            let page_bytes = bulk_bytes.slice(start..end);
            let page = HeapPageData::parse_bytes(page_bytes)?;
            let batch = page.to_record_batch(&self.schema, proj_slice);
            self.buffer.push_back(batch.unwrap());
        }
        self.next_page_index += pages_read;
        if pages_read < want {
            self.exhausted = true;
        }

        // let batches = convert_pages_parallel(&pages, &self.schema, proj_slice, self.num_threads)?;
        // self.buffer.extend(batches);
        Ok(())
    }
}

impl<R: ChunkReader> Iterator for RecordBatchStream<R> {
    type Item = Result<arrow::record_batch::RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        // Drain buffer first.
        if let Some(batch) = self.buffer.pop_front() {
            return Some(Ok(batch));
        }

        // Buffer empty — try to fill it.
        if self.exhausted {
            return None;
        }

        if let Err(e) = self.fill_buffer() {
            return Some(Err(e));
        }

        self.buffer.pop_front().map(Ok)
    }
}

/// Convert a slice of pages to `RecordBatch`es, using parallel threads when
/// beneficial. Pages are partitioned across threads; each thread converts
/// its chunk sequentially. Results are returned in page order.
fn convert_pages_parallel(
    pages: &[HeapPageData],
    schema: &crate::types::PgSchema,
    projection: Option<&[usize]>,
    num_threads: usize,
) -> Result<Vec<arrow::record_batch::RecordBatch>> {
    if pages.is_empty() {
        return Ok(Vec::new());
    }

    // Not worth spawning threads for tiny batches.
    let effective_threads = num_threads.min(pages.len());
    if effective_threads <= 1 || pages.len() <= 2 {
        let mut batches = Vec::with_capacity(pages.len());
        for page in pages {
            batches.push(page.to_record_batch(schema, projection)?);
        }
        return Ok(batches);
    }

    let chunk_size = pages.len().div_ceil(effective_threads);
    let mut results: Vec<Result<Vec<arrow::record_batch::RecordBatch>>> =
        Vec::with_capacity(effective_threads);

    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(effective_threads);

        for chunk in pages.chunks(chunk_size) {
            let handle = s.spawn(move || {
                let mut batches = Vec::with_capacity(chunk.len());
                for page in chunk {
                    batches.push(page.to_record_batch(schema, projection)?);
                }
                Ok(batches)
            });
            handles.push(handle);
        }

        for handle in handles {
            results.push(handle.join().unwrap());
        }
    });

    let mut batches = Vec::with_capacity(pages.len());
    for result in results {
        batches.extend(result?);
    }
    Ok(batches)
}

impl<R: ChunkReader> IntoIterator for PageReader<R> {
    type Item = Result<HeapTupleData>;
    type IntoIter = PageRowIter<R>;

    fn into_iter(self) -> Self::IntoIter {
        PageRowIter {
            page_reader: self,
            current_row_info: None,
            current_page: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RowInfo {
    // Row tid
    ctid: ItemPointerData,
}

pub struct PageRowIter<R: ChunkReader> {
    page_reader: PageReader<R>,
    current_row_info: Option<RowInfo>,
    current_page: Option<HeapPageData>,
}

impl<R: ChunkReader> Iterator for PageRowIter<R> {
    type Item = Result<HeapTupleData>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut current_page = if let Some(current_page) = &self.current_page {
                current_page
            } else if let Ok(page) = self.page_reader.get_page_by_index(0) {
                self.current_page = Some(page);
                self.current_page.as_ref().unwrap()
            } else {
                return None;
            };

            if self.current_row_info.is_some() {
                let current_row_info = self.current_row_info.as_mut().unwrap();
                // fetch next page if finished with the current one
                if current_page.lp_num <= current_row_info.ctid.ip_posid as usize + 1 {
                    let next_page_block_number =
                        current_row_info.ctid.ip_blkid.block_number() as usize;
                    let page = self
                        .page_reader
                        .get_page_by_index(next_page_block_number + 1);

                    if page.is_err() {
                        return None;
                    }

                    self.current_page = Some(page.unwrap());
                    current_page = self.current_page.as_ref().unwrap();
                    current_row_info.ctid.ip_blkid.bi_lo = (next_page_block_number + 1) as u16;
                    current_row_info.ctid.ip_blkid.bi_hi =
                        ((next_page_block_number + 1) as u32 >> 16) as u16;
                    // u16::MAX wraps to 0 via wrapping_add(1) below, so slot 0
                    // of the new page is not skipped.
                    current_row_info.ctid.ip_posid = u16::MAX;
                }
            }

            self.current_row_info = Some(RowInfo {
                ctid: ItemPointerData {
                    ip_blkid: BlockIdData {
                        bi_hi: if let Some(row_info) = self.current_row_info.as_ref() {
                            row_info.ctid.ip_blkid.bi_hi
                        } else {
                            0
                        },
                        bi_lo: if let Some(row_info) = self.current_row_info.as_ref() {
                            row_info.ctid.ip_blkid.bi_lo
                        } else {
                            0
                        },
                    },
                    ip_posid: if let Some(_row_info) = self.current_row_info.as_ref() {
                        self.current_row_info.as_ref().unwrap().ctid.ip_posid.wrapping_add(1)
                    } else {
                        0
                    },
                },
            });

            let ip_posid = self.current_row_info.as_ref().unwrap().ctid.ip_posid;
            let row_data = current_page.get_row_data(ip_posid);
            match row_data {
                Ok(row) => return Some(Ok(row)),
                Err(pg_error) => match pg_error {
                    PgError::DeadTupleLinePointer { ip_posid: _ } => {
                        log::debug!("Got error: {}, skipping the row ", pg_error);
                        continue;
                    }
                    error => {
                        log::error!("Got unexpected error {}", error);
                    }
                },
            };
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// AsyncBatchStream — async wrapper with single spawn_blocking per batch
// ────────────────────────────────────────────────────────────────────────────

/// Async stream that offloads each I/O + conversion batch to a single
/// `spawn_blocking` call.
///
/// `tokio::fs` is internally `spawn_blocking` per syscall, so 128 async page
/// reads = 128 blocking task spawns. Instead, we do one `spawn_blocking` that
/// reads all pages for the batch AND converts them to `RecordBatch`es, then
/// yields the results back to the async context. This minimizes task
/// scheduling overhead while keeping the stream non-blocking.
pub struct AsyncBatchStream {
    /// The synchronous `RecordBatchtream` moved into blocking tasks.
    /// `None` while an in-flight blocking task owns it.
    inner: Option<RecordBatchStream<TableFileReader>>,

    buffer: std::collections::VecDeque<arrow::record_batch::RecordBatch>,
    exhausted: bool,
}

/// Result returned from the blocking fill task.
type FillResult = (
    RecordBatchStream<TableFileReader>,
    std::result::Result<Vec<arrow::record_batch::RecordBatch>, PgError>,
    bool, // exhausted
);

impl AsyncBatchStream {
    pub fn new(
        db_id: Oid,
        relation_id: Oid,
        schema: crate::types::PgSchema,
        projection: Option<Vec<usize>>,
    ) -> Self {
        let reader = TableFileReader::new(db_id, relation_id);
        let page_reader = reader.get_page_reader().unwrap();
        let inner = RecordBatchStream::new(page_reader, schema, projection);
        Self {
            inner: Some(inner),
            buffer: std::collections::VecDeque::new(),
            exhausted: false,
        }
    }

    pub fn with_pages_per_batch(mut self, n: usize) -> Self {
        if let Some(inner) = self.inner.as_mut() {
            inner.pages_per_batch = if n == 0 { DEFAULT_PAGES_PER_BATCH } else { n };
        }
        self
    }

    pub fn with_page_range(mut self, start: usize, end: usize) -> Self {
        if let Some(inner) = self.inner.as_mut() {
            inner.next_page_index = start;
            inner.max_page_index = Some(end);
        }
        self
    }

    /// Get the next `RecordBatch`. If the buffer is empty, offloads a full
    /// I/O + conversion batch to `spawn_blocking` (one task per batch, not
    /// per page).
    pub async fn next_batch(&mut self) -> Option<Result<arrow::record_batch::RecordBatch>> {
        // Drain buffered batches first — no blocking needed.
        if let Some(batch) = self.buffer.pop_front() {
            return Some(Ok(batch));
        }

        if self.exhausted {
            return None;
        }

        // Take ownership of the sync stream for the blocking task.
        let mut stream = self.inner.take()?;

        let result: std::result::Result<FillResult, _> = tokio::task::spawn_blocking(move || {
            // Read pages + convert to RecordBatches in one blocking call.
            if let Err(e) = stream.fill_buffer() {
                let exhausted = stream.exhausted;
                return (stream, Err(e), exhausted);
            }
            let batches: Vec<_> = stream.buffer.drain(..).collect();
            let exhausted = stream.exhausted;
            (stream, Ok(batches), exhausted)
        })
        .await;

        match result {
            Ok((stream, Ok(batches), exhausted)) => {
                self.inner = Some(stream);
                self.exhausted = exhausted && batches.is_empty();
                self.buffer.extend(batches);
                self.buffer.pop_front().map(Ok)
            }
            Ok((stream, Err(e), _)) => {
                self.inner = Some(stream);
                self.exhausted = true;
                Some(Err(e))
            }
            Err(join_err) => {
                self.exhausted = true;
                Some(Err(PgError::DecodeError(join_err.to_string())))
            }
        }
    }
}
