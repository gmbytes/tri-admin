use admin_api::modules::data_tools::formats::{self, Cell, Format, Table};

#[test]
fn csv_roundtrip_handles_unicode_quotes_multiline_and_typed_exports() {
    let table = Table {
        headers: vec!["name".into(), "value".into()],
        rows: vec![
            vec![Cell::Text("中文,\"\n说明".into()), Cell::Number(3.5)],
            vec![Cell::Text("enabled".into()), Cell::Boolean(true)],
        ],
    };
    let bytes = formats::encode(Format::Csv, &table).unwrap();
    let parsed = formats::decode(Format::Csv, &bytes).unwrap();
    assert_eq!(parsed.headers, table.headers);
    assert_eq!(parsed.rows[0][0], table.rows[0][0]);
    assert_eq!(parsed.rows[0][1], Cell::Text("3.5".into()));
    assert_eq!(parsed.rows[1][1], Cell::Text("true".into()));
}

#[test]
fn xlsx_roundtrip_preserves_literal_formula_text_types_and_leading_zeroes() {
    let table = Table {
        headers: vec!["text".into(), "number".into(), "boolean".into()],
        rows: vec![
            vec![
                Cell::Text("=HYPERLINK(\"https://example.invalid\")".into()),
                Cell::Number(12.5),
                Cell::Boolean(false),
            ],
            vec![Cell::Text("00123".into()), Cell::Empty, Cell::Empty],
        ],
    };
    let bytes = formats::encode(Format::Xlsx, &table).unwrap();
    assert!(bytes.starts_with(b"PK"));
    assert_eq!(formats::decode(Format::Xlsx, &bytes).unwrap(), table);
}

#[test]
fn csv_formula_prefixes_are_neutralized_and_unsafe_headers_rejected() {
    for value in ["=1+1", "+cmd", "-2+1", "@SUM(A1)", "\t=1", " \r=1"] {
        let table = Table {
            headers: vec!["value".into()],
            rows: vec![vec![Cell::Text(value.into())]],
        };
        let bytes = formats::encode(Format::Csv, &table).unwrap();
        let read = formats::decode(Format::Csv, &bytes).unwrap();
        assert_eq!(read.rows[0][0], Cell::Text(format!("'{value}")));
    }
    assert!(formats::decode(Format::Csv, b"name,name\na,b\n").is_err());
    assert!(formats::decode(Format::Csv, b"name,value\na\n").is_err());
    assert!(formats::decode(Format::Csv, &[0xff, 0xfe]).is_err());
}

#[test]
fn file_rows_columns_cells_and_safe_numbers_are_bounded() {
    assert!(formats::decode(Format::Csv, &vec![b'x'; formats::MAX_FILE_BYTES + 1]).is_err());
    let many = format!("value\n{}", "x\n".repeat(formats::MAX_ROWS + 1));
    assert!(formats::decode(Format::Csv, many.as_bytes()).is_err());
    let cell = format!("value\n{}\n", "x".repeat(formats::MAX_CELL_BYTES + 1));
    assert!(formats::decode(Format::Csv, cell.as_bytes()).is_err());
    let table = Table {
        headers: vec!["value".into()],
        rows: vec![vec![Cell::Number(f64::INFINITY)]],
    };
    assert!(formats::encode(Format::Xlsx, &table).is_err());
    let columns = (0..33)
        .map(|i| format!("column{i}"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(formats::decode(Format::Csv, columns.as_bytes()).is_err());
}

#[test]
fn xlsx_formulas_sparse_out_of_bounds_dimensions_and_multiple_sheets_are_rejected() {
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 0, "value").unwrap();
    sheet.write_formula(1, 0, "=1+1").unwrap();
    assert!(formats::decode(Format::Xlsx, &book.save_to_buffer().unwrap()).is_err());
    let mut book = rust_xlsxwriter::Workbook::new();
    book.add_worksheet()
        .write_string(1_000_000, 0, "oversized sparse cell")
        .unwrap();
    assert!(formats::decode(Format::Xlsx, &book.save_to_buffer().unwrap()).is_err());
    let mut book = rust_xlsxwriter::Workbook::new();
    book.add_worksheet().write_string(0, 0, "one").unwrap();
    book.add_worksheet().write_string(0, 0, "two").unwrap();
    assert!(formats::decode(Format::Xlsx, &book.save_to_buffer().unwrap()).is_err());
}

#[test]
fn xlsx_relationship_cannot_bypass_sheet_bounds_with_a_nonstandard_part_name() {
    use std::io::{Cursor, Read, Write};
    let mut book = rust_xlsxwriter::Workbook::new();
    book.add_worksheet()
        .write_string(1_000_000, 0, "sparse")
        .unwrap();
    let bytes = book.save_to_buffer().unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).unwrap();
        let target = if name == "xl/worksheets/sheet1.xml" {
            "xl/unsafe.xml"
        } else {
            &name
        };
        if name == "xl/_rels/workbook.xml.rels" {
            contents = String::from_utf8(contents)
                .unwrap()
                .replace("worksheets/sheet1.xml", "unsafe.xml")
                .into_bytes();
        }
        output
            .start_file(target, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&contents).unwrap();
    }
    let bytes = output.finish().unwrap().into_inner();
    assert!(formats::decode(Format::Xlsx, &bytes).is_err());
}
