use serde_json::json;
use tri_resource_generator::render::render;
use tri_resource_runtime::schema::Schema;
#[test]
fn full_project_is_deterministic_safe_and_has_matching_manifests() {
    let schema=Schema::parse(json!({"resource":"inventory","label":"库存 \" </script><script>unsafe()</script>","fields":[{"name":"name","label":"<img onerror=unsafe()>","kind":"text","required":true}]})).unwrap();
    let a = render(&schema).unwrap();
    let b = render(&schema).unwrap();
    assert_eq!(a.archive, b.archive);
    assert_eq!(a.sha256, b.sha256);
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "server/src/lib.rs",
        "server/src/main.rs",
        "server/tests/http.rs",
        "runtime/src/auth.rs",
        "web/package.json",
        "web/package-lock.json",
        "web/src/App.vue",
        "web/src/client.mjs",
        "web/tests/client.test.mjs",
        "README.md",
        "schema.json",
        "sample.json",
        "tools/init-env.mjs",
    ] {
        assert!(a.files.iter().any(|file| file.path == path), "{path}");
    }
    for file in &a.files {
        assert!(
            !file.path.contains("..") && !file.path.contains('\\') && !file.path.starts_with('/')
        );
        assert!(
            !file.content.contains("E:\\hygd") && !file.content.contains("FixtureAdminPassword")
        );
    }
    let app = &a
        .files
        .iter()
        .find(|file| file.path == "web/src/App.vue")
        .unwrap()
        .content;
    assert!(!app.contains("unsafe()"));
    assert!(app.contains("field.label"));
    let manifest = &a
        .files
        .iter()
        .find(|file| file.path == "manifest.json")
        .unwrap()
        .content;
    let manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
    assert_eq!(manifest["templateVersion"], 1);
    assert_eq!(manifest["schemaChecksum"], schema.checksum().unwrap());
    let cursor = std::io::Cursor::new(&a.archive);
    let archive = zip::ZipArchive::new(cursor).unwrap();
    assert_eq!(archive.len(), a.files.len());
}
