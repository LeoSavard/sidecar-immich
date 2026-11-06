// SPDX-License-Identifier: Apache-2.0
//! Uniform structs written once, in Rust. `#[derive(Uniform)]` on a `#[repr(C)]` struct gives its WGSL
//! declaration, which lives in `shaders/uniforms/<snake_name>.wgsl` — generated, committed, and pulled into a
//! shader with `//! requires: uniforms/<snake_name>.wgsl` like any module. A crate's `uniforms` test (feature
//! `check`) fails when a file has drifted from its struct and checks every offset with naga; with
//! `SIDECAR_WRITE_UNIFORMS=1` it rewrites the files instead (`npm run uniforms` in `web/` does that).

pub use sidecar_uniform_derive::Uniform;

/// A member as WGSL declares it, with its offset in the Rust struct.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub offset: usize,
}

pub trait Uniform {
    /// The struct's name, in Rust and in WGSL.
    const NAME: &'static str;
    /// The Rust path it is generated from.
    const SOURCE: &'static str;
    const SIZE: usize;
    fn fields() -> Vec<Field>;
    /// `struct Name { … }`, with each field's documentation as comments.
    fn declaration() -> String;
    /// The uniform structs this one contains.
    fn requires() -> Vec<Requirement>;
}

/// A uniform type, held without its type parameter so a list can mix them.
#[derive(Clone, Copy)]
pub struct Requirement {
    pub name: &'static str,
    pub source: &'static str,
    pub size: usize,
    pub fields: fn() -> Vec<Field>,
    pub declaration: fn() -> String,
    pub requires: fn() -> Vec<Requirement>,
}

impl Requirement {
    pub fn of<T: Uniform>() -> Self {
        Self {
            name: T::NAME,
            source: T::SOURCE,
            size: T::SIZE,
            fields: T::fields,
            declaration: T::declaration,
            requires: T::requires,
        }
    }

    /// `uniforms/develop_params.wgsl` for `DevelopParams`: the path a shader requires, under `shaders/`.
    pub fn path(&self) -> String {
        let mut snake = String::new();
        for (i, c) in self.name.chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        }
        format!("uniforms/{snake}.wgsl")
    }

    /// The generated file.
    pub fn file(&self) -> String {
        let mut out = String::from("// SPDX-License-Identifier: Apache-2.0\n");
        out.push_str(&format!(
            "// Generated from `{}` by #[derive(Uniform)].\n\
             // Do not edit: change the Rust struct, then run `npm run uniforms` in web/.\n",
            self.source
        ));
        for r in (self.requires)() {
            out.push_str(&format!("//! requires: {}\n", r.path()));
        }
        out.push('\n');
        out.push_str(&(self.declaration)());
        out
    }
}

#[cfg(feature = "check")]
pub mod check {
    use super::Requirement;
    use std::path::Path;

    /// Every declaration `r` needs, dependencies first, then its own.
    fn source(r: &Requirement, out: &mut String, done: &mut Vec<&'static str>) {
        if done.contains(&r.name) {
            return;
        }
        for dep in (r.requires)() {
            source(&dep, out, done);
        }
        done.push(r.name);
        out.push_str(&(r.declaration)());
    }

    /// For each uniform: its file under `shaders` matches what the struct generates (or is rewritten, with
    /// `SIDECAR_WRITE_UNIFORMS` set), and naga lays the declaration out exactly as Rust does.
    pub fn uniforms(shaders: &Path, list: &[Requirement]) {
        let write = std::env::var_os("SIDECAR_WRITE_UNIFORMS").is_some();
        for r in list {
            let path = shaders.join(r.path());
            let want = r.file();
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            if have != want {
                if write {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, &want).unwrap();
                } else {
                    panic!(
                        "shaders/{} is not what `{}` generates: run `npm run uniforms` in web/",
                        r.path(),
                        r.source
                    );
                }
            }
            layout(r);
        }
    }

    fn layout(r: &Requirement) {
        let mut src = String::new();
        source(r, &mut src, &mut Vec::new());
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{}: {}", r.name, e.emit_to_string(&src)));
        let ty = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(r.name))
            .map(|(_, t)| t)
            .unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("{} is not a struct", r.name)
        };
        let wgsl: Vec<_> = members
            .iter()
            .map(|m| (m.name.clone().unwrap_or_default(), m.offset as usize))
            .collect();
        let rust: Vec<_> = (r.fields)()
            .into_iter()
            .map(|f| (f.name, f.offset))
            .collect();
        assert_eq!(
            wgsl, rust,
            "{}: WGSL offsets (left) against Rust's (right)",
            r.name
        );
        assert_eq!(
            *span as usize, r.size,
            "{}: WGSL size against Rust's",
            r.name
        );
    }
}
