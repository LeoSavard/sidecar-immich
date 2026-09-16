// SPDX-License-Identifier: Apache-2.0
// Embeds every file under shaders/ so composition needs no filesystem access at run time.
use std::{env, fs, path::Path};

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, root, out);
        } else if p.extension().is_some_and(|e| e == "wgsl") {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, p.canonicalize().unwrap().to_string_lossy().into_owned()));
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders").canonicalize().unwrap();
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    let mut code = String::from("pub static FILES: &[(&str, &str)] = &[\n");
    for (rel, abs) in &files {
        println!("cargo:rerun-if-changed={abs}");
        code.push_str(&format!("    ({rel:?}, include_str!({abs:?})),\n"));
    }
    code.push_str("];\n");
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("files.rs"), code).unwrap();
}
