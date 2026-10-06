use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::{env, fs, io::Write, path::Path};

fn main() {
    let out = env::var_os("OUT_DIR").expect("Cargo output directory");
    let files = [
        "index.html",
        "app.js",
        "style.css",
        "assets/geist-latin.woff2",
        "assets/geist-mono-latin.woff2",
        "assets/instrument-serif-latin.woff2",
        "assets/zeron-favicon-v3.png",
    ];
    let mut assets = String::from("pub static ASSETS: &[Asset] = &[\n");
    for file in files {
        let path = format!("public/{file}");
        println!("cargo:rerun-if-changed={path}");
        let bytes = fs::read(&path).expect("read static asset");
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let name = file.replace('/', "_");
        let mut compressed = GzEncoder::new(Vec::new(), Compression::best());
        compressed.write_all(&bytes).expect("compress asset");
        fs::write(
            Path::new(&out).join(format!("{name}.gz")),
            compressed.finish().expect("finish gzip"),
        )
        .expect("write gzip");
        let mime = match file.rsplit('.').next().unwrap() {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "woff2" => "font/woff2",
            "png" => "image/png",
            _ => unreachable!(),
        };
        let route = if file == "index.html" {
            "/".to_owned()
        } else {
            format!("/{file}")
        };
        let etag = format!("W/\"{hash}\"");
        assets.push_str(&format!("Asset {{ path: {route:?}, mime: {mime:?}, etag: {etag:?}, raw: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{path}\")), gzip: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{name}.gz\")) }},\n"));
    }
    assets.push_str("];\n");
    fs::write(Path::new(&out).join("assets.rs"), assets).expect("write asset index");
}
