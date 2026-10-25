use admin_api::modules::data_tools::{
    export_writer::{ExportError, ExportLimits, ExportWriter},
    formats::{Cell, Format},
};
use calamine::{Data, Reader, Xlsx};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, Write},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn output(directory: &std::path::Path) -> std::fs::File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join("result.part"))
        .unwrap()
}
fn headers() -> Vec<String> {
    ["name", "value", "enabled"].map(String::from).to_vec()
}
fn limits() -> ExportLimits {
    ExportLimits::default()
}

#[test]
fn csv_streams_beyond_the_import_limit_and_verifies_actual_disk_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = ExportWriter::new(
        Format::Csv,
        headers(),
        output(directory.path()),
        directory.path(),
        limits(),
        CancellationToken::new(),
    )
    .unwrap();
    for row in 0..5001 {
        writer
            .write_row(&[
                Cell::Text(format!("=row-{row}")),
                Cell::Number(row as f64),
                Cell::Boolean(false),
            ])
            .unwrap();
    }
    assert!(
        std::fs::metadata(directory.path().join("result.part"))
            .unwrap()
            .len()
            > 0
    );
    let mut artifact = writer.finish().unwrap();
    assert_eq!(artifact.row_count, 5001);
    let mut bytes = Vec::new();
    artifact.file.read_to_end(&mut bytes).unwrap();
    assert!(bytes.starts_with(b"name,value,enabled\n"));
    assert_eq!(artifact.spec.size(), bytes.len() as u64);
    assert_eq!(
        artifact.spec.sha256(),
        format!("{:x}", Sha256::digest(&bytes))
    );
    let rows = csv::Reader::from_reader(bytes.as_slice())
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 5001);
    assert_eq!(&rows[0][0], "'=row-0");
    assert_eq!(&rows[5000][1], "5000");
    assert_eq!(&rows[5000][2], "false");
}

#[test]
fn changed_or_truncated_disk_content_is_not_published_as_a_different_export() {
    for truncate in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = ExportWriter::new(
            Format::Csv,
            headers(),
            output(directory.path()),
            directory.path(),
            limits(),
            CancellationToken::new(),
        )
        .unwrap();
        for _ in 0..5001 {
            writer
                .write_row(&[
                    Cell::Text("row".into()),
                    Cell::Number(1.0),
                    Cell::Boolean(true),
                ])
                .unwrap();
        }
        let mut changed = OpenOptions::new()
            .write(true)
            .open(directory.path().join("result.part"))
            .unwrap();
        if truncate {
            changed.set_len(1).unwrap();
        } else {
            changed.write_all(b"NAME").unwrap();
        }
        changed.sync_all().unwrap();
        drop(changed);
        assert_eq!(
            writer.finish().err().unwrap(),
            ExportError::IntegrityMismatch
        );
    }
}

#[test]
fn xlsx_streams_typed_rows_with_literal_formulas_and_inline_strings() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = ExportWriter::new(
        Format::Xlsx,
        headers(),
        output(directory.path()),
        directory.path(),
        limits(),
        CancellationToken::new(),
    )
    .unwrap();
    for row in 0..5001 {
        writer
            .write_row(&[
                Cell::Text(format!("=SUM({row},1)")),
                Cell::Number(row as f64),
                Cell::Boolean(row % 2 == 0),
            ])
            .unwrap();
    }
    let artifact = writer.finish().unwrap();
    assert_eq!(artifact.row_count, 5001);
    let mut archive = zip::ZipArchive::new(artifact.file.try_clone().unwrap()).unwrap();
    assert!(archive.by_name("xl/sharedStrings.xml").is_err());
    let mut sheet = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    assert!(sheet.contains("inlineStr"));
    assert!(!sheet.contains("<f>"));
    let mut workbook: Xlsx<_> = Xlsx::new(artifact.file).unwrap();
    let range = workbook.worksheet_range("Sheet1").unwrap();
    assert_eq!(range.height(), 5002);
    assert_eq!(range.get((1, 0)), Some(&Data::String("=SUM(0,1)".into())));
    assert_eq!(range.get((5001, 1)), Some(&Data::Float(5000.0)));
    assert_eq!(range.get((5001, 2)), Some(&Data::Bool(true)));
}

#[test]
fn invalid_headers_and_nonempty_output_are_rejected_without_truncation() {
    for headers in [
        vec![],
        vec!["x".into(), "x".into()],
        vec!["\n".into()],
        vec!["x".repeat(129)],
        vec!["x".into(); 33],
    ] {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            ExportWriter::new(
                Format::Csv,
                headers,
                output(directory.path()),
                directory.path(),
                limits(),
                CancellationToken::new()
            )
            .err()
            .unwrap(),
            ExportError::Invalid
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let mut file = output(directory.path());
    file.write_all(b"keep").unwrap();
    file.rewind().unwrap();
    assert_eq!(
        ExportWriter::new(
            Format::Csv,
            headers(),
            file,
            directory.path(),
            limits(),
            CancellationToken::new()
        )
        .err()
        .unwrap(),
        ExportError::Invalid
    );
    assert_eq!(
        std::fs::read(directory.path().join("result.part")).unwrap(),
        b"keep"
    );
}

#[test]
fn invalid_cells_poison_the_writer_and_cannot_publish_a_valid_prefix() {
    for row in [
        vec![Cell::Empty],
        vec![Cell::Text("x".repeat(16385)), Cell::Empty, Cell::Empty],
        vec![Cell::Number(f64::NAN), Cell::Empty, Cell::Empty],
        vec![
            Cell::Number(9_007_199_254_740_992.0),
            Cell::Empty,
            Cell::Empty,
        ],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = ExportWriter::new(
            Format::Csv,
            headers(),
            output(directory.path()),
            directory.path(),
            limits(),
            CancellationToken::new(),
        )
        .unwrap();
        writer
            .write_row(&[Cell::Empty, Cell::Empty, Cell::Empty])
            .unwrap();
        assert_eq!(writer.write_row(&row).unwrap_err(), ExportError::Invalid);
        assert_eq!(writer.finish().err().unwrap(), ExportError::Invalid);
    }
}

#[test]
fn exceeding_the_requested_row_limit_fails_instead_of_truncating() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = limits();
    settings.max_rows = 1;
    let mut writer = ExportWriter::new(
        Format::Csv,
        headers(),
        output(directory.path()),
        directory.path(),
        settings,
        CancellationToken::new(),
    )
    .unwrap();
    writer
        .write_row(&[Cell::Empty, Cell::Empty, Cell::Empty])
        .unwrap();
    assert_eq!(
        writer
            .write_row(&[Cell::Empty, Cell::Empty, Cell::Empty])
            .unwrap_err(),
        ExportError::RowLimit
    );
    assert_eq!(writer.finish().err().unwrap(), ExportError::RowLimit);
}

#[test]
fn final_output_and_intermediate_budgets_are_enforced_for_both_formats() {
    for format in [Format::Csv, Format::Xlsx] {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = limits();
        settings.max_output_bytes = 32;
        let mut writer = ExportWriter::new(
            format,
            headers(),
            output(directory.path()),
            directory.path(),
            settings,
            CancellationToken::new(),
        )
        .unwrap();
        writer
            .write_row(&[Cell::Text("large".repeat(20)), Cell::Empty, Cell::Empty])
            .unwrap();
        assert_eq!(writer.finish().err().unwrap(), ExportError::OutputLimit);
        assert!(
            std::fs::metadata(directory.path().join("result.part"))
                .unwrap()
                .len()
                <= 32
        );
        let directory = tempfile::tempdir().unwrap();
        let mut settings = limits();
        settings.max_intermediate_bytes = if format == Format::Csv {
            32
        } else {
            128 * 1024
        };
        let mut writer = ExportWriter::new(
            format,
            headers(),
            output(directory.path()),
            directory.path(),
            settings,
            CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            writer
                .write_row(&[Cell::Text("&".repeat(16384)), Cell::Empty, Cell::Empty])
                .unwrap_err(),
            ExportError::IntermediateLimit
        );
        assert_eq!(
            writer.finish().err().unwrap(),
            ExportError::IntermediateLimit
        );
    }
}

#[test]
fn cancellation_and_deadline_never_return_a_partial_artifact() {
    for format in [Format::Csv, Format::Xlsx] {
        let directory = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let mut writer = ExportWriter::new(
            format,
            headers(),
            output(directory.path()),
            directory.path(),
            limits(),
            cancel.clone(),
        )
        .unwrap();
        writer
            .write_row(&[Cell::Text("ready".into()), Cell::Empty, Cell::Empty])
            .unwrap();
        cancel.cancel();
        assert_eq!(
            writer
                .write_row(&[Cell::Empty, Cell::Empty, Cell::Empty])
                .unwrap_err(),
            ExportError::Cancelled
        );
        assert_eq!(writer.finish().err().unwrap(), ExportError::Cancelled);
    }
    let directory = tempfile::tempdir().unwrap();
    let mut settings = limits();
    settings.max_duration = Duration::from_millis(20);
    let writer = ExportWriter::new(
        Format::Csv,
        headers(),
        output(directory.path()),
        directory.path(),
        settings,
        CancellationToken::new(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(writer.finish().err().unwrap(), ExportError::TimedOut);
}

#[test]
fn empty_artifacts_and_cancellation_before_create_or_finish_follow_the_same_contract() {
    for format in [Format::Csv, Format::Xlsx] {
        let directory = tempfile::tempdir().unwrap();
        let mut artifact = ExportWriter::new(
            format,
            headers(),
            output(directory.path()),
            directory.path(),
            limits(),
            CancellationToken::new(),
        )
        .unwrap()
        .finish()
        .unwrap();
        assert_eq!(artifact.row_count, 0);
        let mut bytes = Vec::new();
        artifact.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(artifact.spec.size(), bytes.len() as u64);
        assert_eq!(
            artifact.spec.sha256(),
            format!("{:x}", Sha256::digest(&bytes))
        );
        let directory = tempfile::tempdir().unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            ExportWriter::new(
                format,
                headers(),
                output(directory.path()),
                directory.path(),
                limits(),
                cancelled
            )
            .err()
            .unwrap(),
            ExportError::Cancelled
        );
        let directory = tempfile::tempdir().unwrap();
        let cancelled = CancellationToken::new();
        let writer = ExportWriter::new(
            format,
            headers(),
            output(directory.path()),
            directory.path(),
            limits(),
            cancelled.clone(),
        )
        .unwrap();
        cancelled.cancel();
        assert_eq!(writer.finish().err().unwrap(), ExportError::Cancelled);
    }
}

#[test]
fn xlsx_xml_escaping_stays_within_the_reserved_intermediate_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = limits();
    settings.max_intermediate_bytes = 4_500_000;
    let columns = (0..32).map(|column| format!("field{column}")).collect();
    let mut writer = ExportWriter::new(
        Format::Xlsx,
        columns,
        output(directory.path()),
        directory.path(),
        settings,
        CancellationToken::new(),
    )
    .unwrap();
    writer
        .write_row(&vec![Cell::Text("\0".repeat(16384)); 32])
        .unwrap();
    let artifact = writer.finish().unwrap();
    let mut archive = zip::ZipArchive::new(artifact.file).unwrap();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        expanded += archive.by_index(index).unwrap().size();
    }
    assert!(expanded <= settings.max_intermediate_bytes);
}

#[test]
fn csv_budget_tracks_actual_encoded_bytes_without_an_xlsx_xml_reservation() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = limits();
    settings.max_intermediate_bytes = 1024;
    let mut writer = ExportWriter::new(
        Format::Csv,
        headers(),
        output(directory.path()),
        directory.path(),
        settings,
        CancellationToken::new(),
    )
    .unwrap();
    writer
        .write_row(&[
            Cell::Text("plain".repeat(20)),
            Cell::Number(1.0),
            Cell::Boolean(true),
        ])
        .unwrap();
    let artifact = writer.finish().unwrap();
    assert!(artifact.spec.size() < 1024);
}

#[test]
fn unsafe_or_unbounded_limits_are_not_accepted() {
    for setting in 0..5 {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = limits();
        match setting {
            0 => settings.max_rows = 0,
            1 => settings.max_rows = 1_000_001,
            2 => settings.max_output_bytes = 5 * 1024 * 1024 * 1024 + 1,
            3 => settings.max_intermediate_bytes = 0,
            _ => settings.max_duration = Duration::from_secs(1801),
        }
        assert_eq!(
            ExportWriter::new(
                Format::Csv,
                headers(),
                output(directory.path()),
                directory.path(),
                settings,
                CancellationToken::new()
            )
            .err()
            .unwrap(),
            ExportError::Invalid
        );
    }
}

#[test]
#[ignore = "explicit separate-process memory gate; set TRI_EXPORT_PROBE_ROWS and TRI_EXPORT_PROBE_FORMAT"]
fn export_writer_memory_probe() {
    let rows = std::env::var("TRI_EXPORT_PROBE_ROWS")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!((1..=1_000_000).contains(&rows));
    let name = std::env::var("TRI_EXPORT_PROBE_FORMAT").unwrap();
    let format = match name.as_str() {
        "csv" => Format::Csv,
        "xlsx" => Format::Xlsx,
        _ => panic!("invalid probe format"),
    };
    let directory = tempfile::tempdir().unwrap();
    let mut settings = limits();
    settings.max_rows = rows;
    let mut writer = ExportWriter::new(
        format,
        headers(),
        output(directory.path()),
        directory.path(),
        settings,
        CancellationToken::new(),
    )
    .unwrap();
    for index in 0..rows {
        writer
            .write_row(&[
                Cell::Text(format!("item-{index:08}")),
                Cell::Number(index as f64),
                Cell::Boolean(true),
            ])
            .unwrap();
    }
    let artifact = writer.finish().unwrap();
    assert_eq!(artifact.row_count, rows);
    println!(
        "{}",
        serde_json::json!({"format":name,"rows":rows,"size":artifact.spec.size(),"sha256":artifact.spec.sha256()})
    );
}
