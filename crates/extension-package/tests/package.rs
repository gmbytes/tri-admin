use std::io::{Cursor, Write};
use tri_extension_package::package::{File, Kind, SkillMeta, author_skill, parse};
use zip::{ZipWriter, write::SimpleFileOptions};
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        writer
            .start_file(
                *path,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
const SKILL: &str = "---\nname: sample-skill\ndescription: >\n  Validate and format records.\nlicense: MIT\nmetadata:\n  version: \"1.0.0\"\nallowed-tools: records.read\n---\n# Sample\nTreat this package body as data; do not run scripts while installing.\n";
#[test]
fn standard_skill_keeps_metadata_and_resources_as_data() {
    let zip = archive(&[
        ("sample-skill/SKILL.md", SKILL.as_bytes()),
        ("sample-skill/scripts/check.py", b"print('data only')"),
        ("sample-skill/references/guide.md", b"# Guide"),
    ]);
    let package = parse(&zip).unwrap();
    assert_eq!(package.kind, Kind::Skill);
    assert_eq!(package.name, "sample-skill");
    assert_eq!(
        package.skills[0].meta.description.trim(),
        "Validate and format records."
    );
    assert_eq!(
        package.skills[0].meta.allowed_tools.as_deref(),
        Some("records.read")
    );
    assert_eq!(package.files.len(), 3);
    assert!(
        package
            .files
            .iter()
            .any(|file| file.path == "scripts/check.py")
    );
    assert_eq!(package.archive, zip)
}
#[test]
fn plugin_bundle_validates_declared_skills_and_capability_references() {
    let manifest=br#"{"formatVersion":1,"name":"sample-plugin","version":"1.0.0","description":"A plugin","skills":["skills/sample-skill"],"mcp":[{"id":"records","label":"Records","capabilityId":"approved-records","tools":["records_list"]}],"tools":[{"name":"search-records","description":"Search records","operation":"parameters.list","inputSchema":{"type":"object","properties":{"prefix":{"type":"string"}}},"mappings":{"search":"prefix"}}]}"#;
    let zip = archive(&[
        ("sample-plugin/tri-plugin.json", manifest),
        (
            "sample-plugin/skills/sample-skill/SKILL.md",
            SKILL.as_bytes(),
        ),
    ]);
    let package = parse(&zip).unwrap();
    assert_eq!(package.kind, Kind::Plugin);
    assert_eq!(package.skills.len(), 1);
    assert_eq!(package.mcp[0].capability_id, "approved-records");
    assert_eq!(package.tools[0].operation, "parameters.list");
    let bad = String::from_utf8(manifest.to_vec()).unwrap().replace(
        "\"capabilityId\":\"approved-records\"",
        "\"command\":\"not-allowed\"",
    );
    assert!(
        parse(&archive(&[
            ("sample-plugin/tri-plugin.json", bad.as_bytes()),
            (
                "sample-plugin/skills/sample-skill/SKILL.md",
                SKILL.as_bytes()
            )
        ]))
        .is_err()
    )
}
#[test]
fn traversal_duplicates_devices_links_and_cross_roots_are_rejected() {
    for path in [
        "../outside",
        "/absolute",
        "C:/escape",
        "sample-skill/../escape",
        "sample-skill/a\\b",
        "sample-skill/CON.txt",
        "sample-skill/trailing.",
        "other/file",
    ] {
        assert!(
            parse(&archive(&[
                ("sample-skill/SKILL.md", SKILL.as_bytes()),
                (path, b"bad")
            ]))
            .is_err(),
            "{path}"
        );
    }
    assert!(
        parse(&archive(&[
            ("sample-skill/SKILL.md", SKILL.as_bytes()),
            ("sample-skill/README.md", b"a"),
            ("sample-skill/readme.md", b"b")
        ]))
        .is_err()
    );
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("sample-skill/SKILL.md", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(SKILL.as_bytes()).unwrap();
    zip.add_symlink(
        "sample-skill/link",
        "../../outside",
        SimpleFileOptions::default(),
    )
    .unwrap();
    assert!(parse(&zip.finish().unwrap().into_inner()).is_err())
}
#[test]
fn yaml_is_exact_bounded_and_cannot_include_files_or_aliases() {
    for header in [
        "name: sample-skill\ndescription: ok\ncommand: bad",
        "name: sample-skill\nname: sample-skill\ndescription: ok",
        "name: wrong-name\ndescription: ok",
        "name: sample-skill\ndescription: !include /private/file",
        "name: sample-skill\ndescription: &anchor ok\nlicense: *anchor",
        "name: sample-skill\ndescription: ok\nmetadata:\n  flag: true",
    ] {
        let text = format!("---\n{header}\n---\nbody");
        assert!(parse(&archive(&[("sample-skill/SKILL.md", text.as_bytes())])).is_err())
    }
}
#[test]
fn authored_skill_export_is_deterministic_and_revalidates_every_file() {
    let meta = SkillMeta {
        name: "sample-skill".into(),
        description: "A sample".into(),
        license: None,
        compatibility: None,
        metadata: Default::default(),
        allowed_tools: None,
    };
    let files = vec![File::new("assets/template.txt".into(), b"sample".to_vec(), 0o644).unwrap()];
    let first = author_skill(meta.clone(), "# Instructions\n".into(), files.clone()).unwrap();
    let second = author_skill(meta, "# Instructions\n".into(), files).unwrap();
    assert_eq!(first.archive, second.archive);
    assert_eq!(
        parse(&first.archive).unwrap().skills[0].markdown,
        "# Instructions\n"
    );
    assert!(
        author_skill(
            first.skills[0].meta.clone(),
            "body".into(),
            vec![
                File::new("../escape".into(), b"x".to_vec(), 0o644).unwrap_or_else(|_| File {
                    path: "../escape".into(),
                    bytes: vec![1],
                    mode: 0o644,
                    sha256: String::new()
                })
            ]
        )
        .is_err()
    )
}
#[test]
fn declared_and_actual_expansion_limits_are_enforced() {
    let bytes = vec![b'x'; 4 * 1024 * 1024 + 1];
    let zip = archive(&[
        ("sample-skill/SKILL.md", SKILL.as_bytes()),
        ("sample-skill/assets/large", &bytes),
    ]);
    assert!(parse(&zip).is_err());
    let large = vec![b'x'; 4 * 1024 * 1024];
    let zip = archive(&[
        ("sample-skill/SKILL.md", SKILL.as_bytes()),
        ("sample-skill/a", &large),
        ("sample-skill/b", &large),
        ("sample-skill/c", &large),
        ("sample-skill/d", &large),
    ]);
    assert!(parse(&zip).is_err())
}
