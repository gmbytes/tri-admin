use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};
use tri_resource_runtime::schema::{Kind, Schema};
use zip::{ZipWriter, write::SimpleFileOptions};
pub const TEMPLATE_VERSION: u32 = 2;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Schema(#[from] tri_resource_runtime::schema::SchemaError),
    #[error("生成工程超出限制或不可编码")]
    Invalid,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub path: String,
    pub content: String,
    pub size: usize,
    pub sha256: String,
}
pub struct Bundle {
    pub files: Vec<File>,
    pub archive: Vec<u8>,
    pub sha256: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn render(schema: &Schema) -> Result<Bundle, Error> {
    schema.validate()?;
    let mut source = BTreeMap::<String, String>::new();
    for (path, content) in [
        ("Cargo.toml", include_str!("../templates/Cargo.toml.txt")),
        ("Cargo.lock", include_str!("../templates/Cargo.lock.txt")),
        (
            "runtime/Cargo.toml",
            include_str!("../templates/runtime-Cargo.toml.txt"),
        ),
        (
            "runtime/src/lib.rs",
            include_str!("../../resource-runtime/src/lib.rs"),
        ),
        (
            "runtime/src/schema.rs",
            include_str!("../../resource-runtime/src/schema.rs"),
        ),
        (
            "runtime/src/error.rs",
            include_str!("../../resource-runtime/src/error.rs"),
        ),
        (
            "runtime/src/state.rs",
            include_str!("../../resource-runtime/src/state.rs"),
        ),
        (
            "runtime/src/auth.rs",
            include_str!("../../resource-runtime/src/auth.rs"),
        ),
        (
            "runtime/src/transaction.rs",
            include_str!("../../resource-runtime/src/transaction.rs"),
        ),
        (
            "runtime/src/routes.rs",
            include_str!("../../resource-runtime/src/routes.rs"),
        ),
        (
            "runtime/src/run.rs",
            include_str!("../../resource-runtime/src/run.rs"),
        ),
        (
            "server/Cargo.toml",
            include_str!("../templates/server-Cargo.toml.txt"),
        ),
        (
            "server/src/lib.rs",
            include_str!("../templates/server-lib.rs.txt"),
        ),
        (
            "server/src/main.rs",
            include_str!("../templates/server-main.rs.txt"),
        ),
        (
            "server/tests/http.rs",
            include_str!("../templates/http.rs.txt"),
        ),
        (
            "web/package.json",
            include_str!("../templates/web/package.json"),
        ),
        (
            "web/package-lock.json",
            include_str!("../templates/web/package-lock.json"),
        ),
        (
            "web/index.html",
            include_str!("../templates/web/index.html"),
        ),
        (
            "web/tsconfig.json",
            include_str!("../templates/web/tsconfig.json"),
        ),
        (
            "web/vite.config.ts",
            include_str!("../templates/web/vite.config.ts"),
        ),
        (
            "web/src/main.ts",
            include_str!("../templates/web/src/main.ts"),
        ),
        (
            "web/src/App.vue",
            include_str!("../templates/web/src/App.vue"),
        ),
        (
            "web/src/client.mjs",
            include_str!("../templates/web/src/client.mjs"),
        ),
        (
            "web/src/style.css",
            include_str!("../templates/web/src/style.css"),
        ),
        (
            "web/tests/client.test.mjs",
            include_str!("../templates/web/tests/client.test.mjs"),
        ),
        (
            "tools/init-env.mjs",
            include_str!("../templates/init-env.mjs"),
        ),
        ("README.md", include_str!("../templates/README.md")),
        (".gitignore", ".env\nnode_modules/\ntarget/\nweb/dist/\n"),
    ] {
        source.insert(path.into(), content.into());
    }
    let descriptor = serde_json::to_string_pretty(schema).map_err(|_| Error::Invalid)?;
    source.insert("schema.json".into(), descriptor.clone());
    source.insert("web/src/schema.json".into(), descriptor);
    let mut sample = serde_json::Map::new();
    for field in &schema.fields {
        sample.insert(
            field.name.clone(),
            match field.kind {
                Kind::Text => json!("sample"),
                Kind::Integer => json!(1),
                Kind::Number => json!(1.25),
                Kind::Boolean => json!(true),
                Kind::Datetime => json!("2026-10-26T00:00:00.000Z"),
                Kind::TextList => json!(["sample"]),
            },
        );
    }
    source.insert(
        "sample.json".into(),
        serde_json::to_string_pretty(&sample).map_err(|_| Error::Invalid)?,
    );
    let mut files = source
        .into_iter()
        .map(|(path, content)| File {
            path,
            size: content.len(),
            sha256: hash(content.as_bytes()),
            content,
        })
        .collect::<Vec<_>>();
    let manifest = json!({"templateVersion":TEMPLATE_VERSION,"schemaChecksum":schema.checksum()?,"files":files.iter().map(|file|json!({"path":file.path,"size":file.size,"sha256":file.sha256})).collect::<Vec<_>>()});
    let content = serde_json::to_string_pretty(&manifest).map_err(|_| Error::Invalid)?;
    files.push(File {
        path: "manifest.json".into(),
        size: content.len(),
        sha256: hash(content.as_bytes()),
        content,
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.len() > 64
        || files.iter().any(|file| file.size > 512 * 1024)
        || files.iter().map(|file| file.size).sum::<usize>() > 2 * 1024 * 1024
    {
        return Err(Error::Invalid);
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for file in &files {
        writer
            .start_file(&file.path, options)
            .map_err(|_| Error::Invalid)?;
        writer
            .write_all(file.content.as_bytes())
            .map_err(|_| Error::Invalid)?;
    }
    let archive = writer.finish().map_err(|_| Error::Invalid)?.into_inner();
    if archive.len() > 1024 * 1024 {
        return Err(Error::Invalid);
    }
    Ok(Bundle {
        sha256: hash(&archive),
        files,
        archive,
    })
}
