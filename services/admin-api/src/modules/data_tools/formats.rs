use crate::common::error::AppError;
use calamine::{Data, Reader, Xlsx};
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Cursor, Read},
};

pub const MAX_FILE_BYTES: usize = 6 * 1024 * 1024;
pub const MAX_ROWS: usize = 2000;
pub const MAX_COLUMNS: usize = 32;
pub const MAX_CELL_BYTES: usize = 16 * 1024;
const MAX_EXPANDED: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Xlsx,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    Number(f64),
    Boolean(bool),
}
impl Cell {
    pub fn text(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(value) => value.clone(),
            Self::Number(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}
fn invalid() -> AppError {
    AppError::Validation("表格格式、类型或大小无效".into())
}
pub(super) fn validate_headers(headers: &[String]) -> Result<(), AppError> {
    if headers.is_empty()
        || headers.len() > MAX_COLUMNS
        || headers
            .iter()
            .any(|v| v.trim().is_empty() || v.len() > 128 || v.chars().any(char::is_control))
        || headers.iter().collect::<HashSet<_>>().len() != headers.len()
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn validate_row(row: &[Cell], columns: usize) -> Result<(), AppError> {
    if row.len() != columns {
        return Err(invalid());
    }
    for cell in row {
        if matches!(cell, Cell::Number(v) if !v.is_finite() || v.abs() > 9_007_199_254_740_991.0) {
            return Err(invalid());
        }
        let size = match cell {
            Cell::Text(text) => text.len(),
            _ => cell.text().len(),
        };
        if size > MAX_CELL_BYTES {
            return Err(invalid());
        }
    }
    Ok(())
}
fn validate(table: &Table) -> Result<(), AppError> {
    validate_headers(&table.headers)?;
    if table.rows.len() > MAX_ROWS {
        return Err(invalid());
    }
    let mut total = 0;
    for row in &table.rows {
        validate_row(row, table.headers.len())?;
        for cell in row {
            total += match cell {
                Cell::Text(text) => text.len(),
                _ => cell.text().len(),
            };
            if total > 8 * 1024 * 1024 {
                return Err(AppError::PayloadTooLarge);
            }
        }
    }
    Ok(())
}

pub fn decode(format: Format, bytes: &[u8]) -> Result<Table, AppError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let table = match format {
        Format::Csv => csv_read(bytes)?,
        Format::Xlsx => xlsx_read(bytes)?,
    };
    validate(&table)?;
    Ok(table)
}

fn csv_read(bytes: &[u8]) -> Result<Table, AppError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid())?
        .strip_prefix('\u{feff}')
        .unwrap_or(std::str::from_utf8(bytes).map_err(|_| invalid())?);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|_| invalid())?
        .iter()
        .map(String::from)
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        if rows.len() == MAX_ROWS {
            return Err(invalid());
        }
        rows.push(
            record
                .map_err(|_| invalid())?
                .iter()
                .map(|v| Cell::Text(v.into()))
                .collect(),
        );
    }
    Ok(Table { headers, rows })
}

fn coordinate(value: &[u8]) -> Result<(), AppError> {
    let split = value
        .iter()
        .position(u8::is_ascii_digit)
        .ok_or_else(invalid)?;
    let (letters, digits) = value.split_at(split);
    if letters.is_empty() || digits.is_empty() || letters.len() > 2 || digits.len() > 4 {
        return Err(invalid());
    }
    let mut column = 0usize;
    for letter in letters {
        if !letter.is_ascii_uppercase() {
            return Err(invalid());
        }
        column = column * 26 + (letter - b'A' + 1) as usize;
    }
    let row = std::str::from_utf8(digits)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .ok_or_else(invalid)?;
    if column == 0 || column > MAX_COLUMNS || row == 0 || row > MAX_ROWS + 1 {
        return Err(invalid());
    }
    Ok(())
}

// Enforce archive and coordinate limits before calamine constructs any rectangular range.
fn preflight(bytes: &[u8]) -> Result<(), AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if archive.len() > 200 {
        return Err(invalid());
    }
    let mut total = 0u64;
    let mut names = HashSet::new();
    let mut sheets = 0;
    let mut actual_xml = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|_| invalid())?;
        let name = file.name().map_err(|_| invalid())?.into_owned();
        let lower_name = name.to_ascii_lowercase();
        if !names.insert(name.clone())
            || name.contains('\\')
            || name.starts_with('/')
            || name.split('/').any(|s| s == "..")
            || lower_name.contains("vbaproject")
            || lower_name.starts_with("xl/externallinks/")
            || lower_name.starts_with("xl/embeddings/")
        {
            return Err(invalid());
        }
        total = total.checked_add(file.size()).ok_or_else(invalid)?;
        if total > MAX_EXPANDED {
            return Err(AppError::PayloadTooLarge);
        }
        if lower_name.ends_with(".xml") || lower_name.ends_with(".rels") {
            let mut data = Vec::new();
            file.take(MAX_EXPANDED + 1)
                .read_to_end(&mut data)
                .map_err(|_| invalid())?;
            if data.len() as u64 > MAX_EXPANDED {
                return Err(AppError::PayloadTooLarge);
            }
            actual_xml += data.len() as u64;
            if actual_xml > MAX_EXPANDED {
                return Err(AppError::PayloadTooLarge);
            }
            let mut xml = quick_xml::Reader::from_reader(data.as_slice());
            let mut worksheet = false;
            let mut cells = 0;
            let mut rows = 0;
            loop {
                match xml.read_event().map_err(|_| invalid())? {
                    Event::Start(tag) | Event::Empty(tag) => {
                        let local = tag.local_name();
                        if local.as_ref() == "worksheet" {
                            worksheet = true;
                            sheets += 1;
                        }
                        if worksheet && local.as_ref() == "f" {
                            return Err(invalid());
                        }
                        if worksheet && local.as_ref() == "c" {
                            cells += 1;
                            if cells > (MAX_ROWS + 1) * MAX_COLUMNS {
                                return Err(invalid());
                            }
                        }
                        if worksheet && local.as_ref() == "row" {
                            rows += 1;
                            if rows > MAX_ROWS + 1 {
                                return Err(invalid());
                            }
                        }
                        let mut has_coordinate = false;
                        for attribute in tag.attributes() {
                            let attribute = attribute.map_err(|_| invalid())?;
                            if attribute.key.local_name().as_ref() == "TargetMode"
                                && attribute.value.as_ref() == "External"
                            {
                                return Err(invalid());
                            }
                            if worksheet
                                && local.as_ref() == "c"
                                && attribute.key.local_name().as_ref() == "r"
                            {
                                has_coordinate = true;
                                coordinate(attribute.value.as_bytes())?;
                            }
                            if worksheet
                                && local.as_ref() == "dimension"
                                && attribute.key.local_name().as_ref() == "ref"
                            {
                                for value in attribute.value.split(':') {
                                    coordinate(value.as_bytes())?;
                                }
                            }
                            if worksheet
                                && local.as_ref() == "row"
                                && attribute.key.local_name().as_ref() == "r"
                            {
                                let mut value = vec![b'A'];
                                value.extend_from_slice(attribute.value.as_bytes());
                                coordinate(&value)?;
                            }
                        }
                        if worksheet && local.as_ref() == "c" && !has_coordinate {
                            return Err(invalid());
                        }
                    }
                    Event::DocType(_) => return Err(invalid()),
                    Event::Eof => break,
                    _ => (),
                }
            }
        }
    }
    if sheets != 1 {
        return Err(invalid());
    }
    Ok(())
}

fn xlsx_read(bytes: &[u8]) -> Result<Table, AppError> {
    preflight(bytes)?;
    let mut book = Xlsx::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let names = book.sheet_names();
    if names.len() != 1 {
        return Err(invalid());
    }
    let range = book.worksheet_range(&names[0]).map_err(|_| invalid())?;
    if range.start() != Some((0, 0)) || range.height() > MAX_ROWS + 1 || range.width() > MAX_COLUMNS
    {
        return Err(invalid());
    }
    let mut rows = range.rows();
    let headers = rows
        .next()
        .ok_or_else(invalid)?
        .iter()
        .map(|cell| match cell {
            Data::String(value) => Ok(value.clone()),
            _ => Err(invalid()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rows = rows
        .map(|row| {
            row.iter()
                .map(|cell| match cell {
                    Data::Empty => Ok(Cell::Empty),
                    Data::String(v) => Ok(Cell::Text(v.clone())),
                    Data::Float(v) => Ok(Cell::Number(*v)),
                    Data::Int(v) if v.unsigned_abs() <= 9_007_199_254_740_991 => {
                        Ok(Cell::Number(*v as f64))
                    }
                    Data::Bool(v) => Ok(Cell::Boolean(*v)),
                    _ => Err(invalid()),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table { headers, rows })
}

pub(super) fn csv_safe(text: &str) -> String {
    let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
    if text.starts_with(['\t', '\r', '\n']) || trimmed.starts_with(['=', '+', '-', '@']) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}
pub fn encode(format: Format, table: &Table) -> Result<Vec<u8>, AppError> {
    validate(table)?;
    let bytes = match format {
        Format::Csv => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            writer
                .write_record(table.headers.iter().map(|s| csv_safe(s)))
                .map_err(|_| invalid())?;
            for row in &table.rows {
                writer
                    .write_record(row.iter().map(|cell| match cell {
                        Cell::Text(s) => csv_safe(s),
                        _ => cell.text(),
                    }))
                    .map_err(|_| invalid())?;
            }
            writer.into_inner().map_err(|_| invalid())?
        }
        Format::Xlsx => {
            let mut book = rust_xlsxwriter::Workbook::new();
            let sheet = book.add_worksheet();
            for (column, header) in table.headers.iter().enumerate() {
                sheet
                    .write_string(0, column as u16, header)
                    .map_err(|_| invalid())?;
            }
            for (index, row) in table.rows.iter().enumerate() {
                for (column, cell) in row.iter().enumerate() {
                    let (r, c) = ((index + 1) as u32, column as u16);
                    match cell {
                        Cell::Empty => (),
                        Cell::Text(value) => {
                            sheet.write_string(r, c, value).map_err(|_| invalid())?;
                        }
                        Cell::Number(value) => {
                            sheet.write_number(r, c, *value).map_err(|_| invalid())?;
                        }
                        Cell::Boolean(value) => {
                            sheet.write_boolean(r, c, *value).map_err(|_| invalid())?;
                        }
                    }
                }
            }
            book.save_to_buffer().map_err(|_| invalid())?
        }
    };
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn archive_checks_all_worksheet_parts_before_range_allocation() {
        for unsafe_name in ["xl/unsafe.xml", "xl/unsafe.XML"] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, xml) in [
                (
                    "xl/worksheets/decoy.xml",
                    "<worksheet><dimension ref=\"A1\"/><sheetData><row r=\"1\"><c r=\"A1\"/></row></sheetData></worksheet>",
                ),
                (
                    unsafe_name,
                    "<worksheet><dimension ref=\"A1000000\"/><sheetData><row r=\"1000000\"><c r=\"A1000000\"/></row></sheetData></worksheet>",
                ),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(xml.as_bytes()).unwrap();
            }
            assert!(preflight(&writer.finish().unwrap().into_inner()).is_err());
        }
    }
}
