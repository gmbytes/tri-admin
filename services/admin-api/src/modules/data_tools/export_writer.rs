//! Incremental artifact generation for durable exports. The caller supplies an
//! empty private file and a leased private XLSX workspace. Nothing in this module
//! publishes a file or turns an incomplete artifact into a successful result.
use super::formats::{self, Cell, Format};
use crate::modules::files::storage::{BlobSpec, MAX_BLOB_SIZE};
use rust_xlsxwriter::Workbook;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub const MAX_EXPORT_ROWS: u32 = 1_000_000;
const XML_BASE_BUDGET: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    #[error("导出字段、单元格或限制无效")]
    Invalid,
    #[error("导出超过本次行数上限")]
    RowLimit,
    #[error("导出文件超过大小上限")]
    OutputLimit,
    #[error("导出超过临时内容预算")]
    IntermediateLimit,
    #[error("导出已取消")]
    Cancelled,
    #[error("导出已超过执行时限")]
    TimedOut,
    #[error("导出文件读写失败")]
    Io,
    #[error("导出编码失败")]
    Encoding,
    #[error("导出磁盘内容校验失败")]
    IntegrityMismatch,
}

#[derive(Clone, Copy)]
pub struct ExportLimits {
    pub max_rows: u32,
    pub max_output_bytes: u64,
    pub max_intermediate_bytes: u64,
    pub max_duration: Duration,
}
impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_rows: 100_000,
            max_output_bytes: MAX_BLOB_SIZE,
            max_intermediate_bytes: MAX_BLOB_SIZE,
            max_duration: Duration::from_secs(1800),
        }
    }
}
impl ExportLimits {
    fn validate(&self) -> Result<(), ExportError> {
        if !(1..=MAX_EXPORT_ROWS).contains(&self.max_rows)
            || !(1..=MAX_BLOB_SIZE).contains(&self.max_output_bytes)
            || !(1..=MAX_BLOB_SIZE).contains(&self.max_intermediate_bytes)
            || self.max_duration.is_zero()
            || self.max_duration > Duration::from_secs(1800)
        {
            return Err(ExportError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Control {
    cancel: CancellationToken,
    start: Instant,
    duration: Duration,
}
impl Control {
    fn check(&self) -> Result<(), ExportError> {
        if self.cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        if self.start.elapsed() >= self.duration {
            return Err(ExportError::TimedOut);
        }
        Ok(())
    }
}

struct Output {
    file: File,
    count: u64,
    hash: Sha256,
    limit: u64,
    intermediate_limit: Option<u64>,
    control: Control,
    failure: Option<ExportError>,
}
impl Output {
    fn failure(&self) -> ExportError {
        self.failure.unwrap_or(ExportError::Encoding)
    }
    fn reject(&mut self, error: ExportError) -> std::io::Error {
        self.failure = Some(error);
        std::io::Error::other(error)
    }
    fn check(&mut self) -> std::io::Result<()> {
        if let Some(error) = self.failure {
            return Err(std::io::Error::other(error));
        }
        if let Err(error) = self.control.check() {
            return Err(self.reject(error));
        }
        Ok(())
    }
    fn artifact(mut self, row_count: u32) -> Result<ExportArtifact, ExportError> {
        self.control.check()?;
        self.file.flush().map_err(|_| ExportError::Io)?;
        self.file.sync_all().map_err(|_| ExportError::Io)?;
        self.file.rewind().map_err(|_| ExportError::Io)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            self.control.check()?;
            let length = self.file.read(&mut buffer).map_err(|_| ExportError::Io)?;
            if length == 0 {
                break;
            }
            size = size
                .checked_add(length as u64)
                .ok_or(ExportError::OutputLimit)?;
            if size > self.limit {
                return Err(ExportError::OutputLimit);
            }
            hash.update(&buffer[..length]);
        }
        self.control.check()?;
        let expected = self.hash.finalize();
        let actual = hash.finalize();
        if size != self.count || expected != actual {
            return Err(ExportError::IntegrityMismatch);
        }
        self.file.rewind().map_err(|_| ExportError::Io)?;
        let spec =
            BlobSpec::new(size, &format!("{actual:x}")).map_err(|_| ExportError::OutputLimit)?;
        Ok(ExportArtifact {
            file: self.file,
            spec,
            row_count,
        })
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        if (bytes.len() as u64) > self.limit.saturating_sub(self.count) {
            return Err(self.reject(ExportError::OutputLimit));
        }
        if self
            .intermediate_limit
            .is_some_and(|limit| (bytes.len() as u64) > limit.saturating_sub(self.count))
        {
            return Err(self.reject(ExportError::IntermediateLimit));
        }
        let count = match self.file.write(bytes) {
            Ok(count) => count,
            Err(_) => return Err(self.reject(ExportError::Io)),
        };
        self.count += count as u64;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.file.flush().map_err(|_| self.reject(ExportError::Io))
    }
}

enum Encoder {
    Csv(Box<csv::Writer<Output>>),
    Xlsx { book: Box<Workbook>, output: Output },
}
pub struct ExportWriter {
    encoder: Encoder,
    columns: usize,
    rows: u32,
    limits: ExportLimits,
    budget: u64,
    control: Control,
    failure: Option<ExportError>,
}
pub struct ExportArtifact {
    pub file: File,
    pub spec: BlobSpec,
    pub row_count: u32,
}

fn cell_budget(cell: &Cell) -> u64 {
    let length = match cell {
        Cell::Text(value) => value.len(),
        _ => cell.text().len(),
    };
    // XML escaping (including Excel's control-character escapes) expands at
    // most eightfold. Fixed cell/row overhead is reserved separately, before
    // handing the row to the constant-memory encoder.
    128 + (length as u64) * 8
}
impl ExportWriter {
    pub fn new(
        format: Format,
        headers: Vec<String>,
        mut file: File,
        temp_directory: &Path,
        limits: ExportLimits,
        cancel: CancellationToken,
    ) -> Result<Self, ExportError> {
        limits.validate()?;
        formats::validate_headers(&headers).map_err(|_| ExportError::Invalid)?;
        if !temp_directory.is_dir()
            || !file.metadata().map_err(|_| ExportError::Io)?.is_file()
            || file.metadata().map_err(|_| ExportError::Io)?.len() != 0
            || file.stream_position().map_err(|_| ExportError::Io)? != 0
        {
            return Err(ExportError::Invalid);
        }
        let control = Control {
            cancel,
            start: Instant::now(),
            duration: limits.max_duration,
        };
        control.check()?;
        let output = Output {
            file,
            count: 0,
            hash: Sha256::new(),
            limit: limits.max_output_bytes,
            intermediate_limit: (format == Format::Csv).then_some(limits.max_intermediate_bytes),
            control: control.clone(),
            failure: None,
        };
        let budget = if format == Format::Xlsx {
            XML_BASE_BUDGET
                + 128
                + headers
                    .iter()
                    .map(|h| 128 + (h.len() as u64) * 8)
                    .sum::<u64>()
        } else {
            0
        };
        if budget > limits.max_intermediate_bytes {
            return Err(ExportError::IntermediateLimit);
        }
        let encoder = match format {
            Format::Csv => {
                let mut writer = csv::Writer::from_writer(output);
                if writer
                    .write_record(headers.iter().map(|header| formats::csv_safe(header)))
                    .is_err()
                {
                    return Err(writer.get_ref().failure());
                }
                Encoder::Csv(Box::new(writer))
            }
            Format::Xlsx => {
                let mut book = Box::new(Workbook::new());
                book.set_tempdir(temp_directory)
                    .map_err(|_| ExportError::Io)?;
                let sheet = book
                    .add_worksheet_with_constant_memory()
                    .map_err(|_| ExportError::Io)?;
                for (column, header) in headers.iter().enumerate() {
                    sheet
                        .write_string(0, column as u16, header)
                        .map_err(|_| ExportError::Encoding)?;
                }
                Encoder::Xlsx { book, output }
            }
        };
        Ok(Self {
            encoder,
            columns: headers.len(),
            rows: 0,
            limits,
            budget,
            control,
            failure: None,
        })
    }
    pub fn write_row(&mut self, row: &[Cell]) -> Result<(), ExportError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.write_inner(row);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    fn write_inner(&mut self, row: &[Cell]) -> Result<(), ExportError> {
        self.control.check()?;
        if self.rows >= self.limits.max_rows {
            return Err(ExportError::RowLimit);
        }
        formats::validate_row(row, self.columns).map_err(|_| ExportError::Invalid)?;
        let cost = if matches!(self.encoder, Encoder::Xlsx { .. }) {
            128 + row.iter().map(cell_budget).sum::<u64>()
        } else {
            0
        };
        if cost
            > self
                .limits
                .max_intermediate_bytes
                .saturating_sub(self.budget)
        {
            return Err(ExportError::IntermediateLimit);
        }
        match &mut self.encoder {
            Encoder::Csv(writer) => {
                if writer
                    .write_record(row.iter().map(|cell| match cell {
                        Cell::Text(value) => formats::csv_safe(value),
                        _ => cell.text(),
                    }))
                    .is_err()
                {
                    return Err(writer.get_ref().failure());
                }
            }
            Encoder::Xlsx { book, .. } => {
                let sheet = book
                    .worksheet_from_index(0)
                    .map_err(|_| ExportError::Encoding)?;
                for (column, cell) in row.iter().enumerate() {
                    let position = (self.rows + 1, column as u16);
                    match cell {
                        Cell::Empty => (),
                        Cell::Text(value) => {
                            sheet
                                .write_string(position.0, position.1, value)
                                .map_err(|_| ExportError::Encoding)?;
                        }
                        Cell::Number(value) => {
                            sheet
                                .write_number(position.0, position.1, *value)
                                .map_err(|_| ExportError::Encoding)?;
                        }
                        Cell::Boolean(value) => {
                            sheet
                                .write_boolean(position.0, position.1, *value)
                                .map_err(|_| ExportError::Encoding)?;
                        }
                    }
                }
            }
        }
        self.budget += cost;
        self.rows += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<ExportArtifact, ExportError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.control.check()?;
        let output = match self.encoder {
            Encoder::Csv(mut writer) => {
                if writer.flush().is_err() {
                    return Err(writer.get_ref().failure());
                }
                (*writer)
                    .into_inner()
                    .map_err(|error| error.into_inner().get_ref().failure())?
            }
            Encoder::Xlsx {
                mut book,
                mut output,
            } => {
                if book.save_to_writer(&mut output).is_err() {
                    return Err(output.failure());
                }
                drop(book);
                output
            }
        };
        output.artifact(self.rows)
    }
}
