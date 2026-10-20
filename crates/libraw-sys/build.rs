// SPDX-License-Identifier: Apache-2.0
// Compiles the LibRaw sources listed in LIB_OBJECTS of third_party/libraw/Makefile.dist, plus shim/shim.cpp.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find(&p, name) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|f| f == name) {
            return Some(p);
        }
    }
    None
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .join("../../third_party/libraw")
        .canonicalize()
        .expect("third_party/libraw submodule");
    let makefile = fs::read_to_string(root.join("Makefile.dist")).expect("LibRaw Makefile.dist");
    let block = makefile
        .split("LIB_OBJECTS=")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();
    let objects: Vec<&str> = block
        .split(|c: char| c.is_whitespace() || c == '\\')
        .filter_map(|t| t.strip_prefix("object/")?.strip_suffix(".o"))
        .collect();
    assert!(objects.len() > 50, "unexpected LibRaw object list");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(&root)
        .warnings(false)
        .opt_level(3)
        .define("USE_ZLIB", None)
        .define("USE_JPEG", None)
        .flag("-fopenmp");
    for obj in &objects {
        let src = find(&root.join("src"), &format!("{obj}.cpp"))
            .unwrap_or_else(|| panic!("no source for {obj}"));
        build.file(src);
    }
    build.file(manifest.join("shim/shim.cpp"));
    build.compile("raw");
    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=jpeg");
    // OpenMP parallelizes Fuji, Canon CR3 and Panasonic decompression (libgomp: GPL with the GCC Runtime Library Exception).
    println!("cargo:rustc-link-lib=gomp");
    println!("cargo:rerun-if-changed=shim/shim.cpp");
    println!("cargo:rerun-if-changed=shim/shim.h");
    println!("cargo:rerun-if-changed={}", root.join("libraw").display());
}
