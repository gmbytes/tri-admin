fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: render schema.json new-output-directory [new-archive.zip]".into());
    }
    let bytes = std::fs::read(&args[0])?;
    if bytes.len() > 64 * 1024 {
        return Err("schema too large".into());
    }
    let schema = tri_resource_runtime::schema::Schema::parse(serde_json::from_slice(&bytes)?)?;
    let bundle = tri_resource_generator::render::render(&schema)?;
    let root = std::path::Path::new(&args[1]);
    std::fs::create_dir(root)?;
    for file in bundle.files {
        let path = root.join(file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, file.content)?;
    }
    if let Some(path) = args.get(2) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        file.write_all(&bundle.archive)?;
        file.sync_all()?;
    }
    println!(
        "{}",
        serde_json::json!({"sha256":bundle.sha256,"size":bundle.archive.len()})
    );
    Ok(())
}
