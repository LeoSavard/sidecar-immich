// SPDX-License-Identifier: Apache-2.0
//! WGSL composition. A file declares its dependencies with `//! requires: a.wgsl b.wgsl` lines; composing an
//! entry file emits every dependency once, depth-first, before the file that needs it. The browser loads the
//! composed output written by `sidecar-dev shaders`, so both sides compile byte-identical WGSL.

use anyhow::{Context, Result, bail};

include!(concat!(env!("OUT_DIR"), "/files.rs"));

/// Entry points that the browser needs, composed into `web/src/lib/gpu/wgsl/`.
pub const WEB_ENTRIES: &[&str] = &["client/develop.wgsl", "client/view.wgsl", "client/histogram.wgsl", "client/lens.wgsl"];

pub fn source(path: &str) -> Result<&'static str> {
    FILES.iter().find(|(p, _)| *p == path).map(|(_, s)| *s).with_context(|| format!("unknown shader {path}"))
}

fn requires(src: &str) -> impl Iterator<Item = &str> {
    src.lines()
        .filter_map(|l| l.trim().strip_prefix("//! requires:"))
        .flat_map(|rest| rest.split_whitespace())
}

pub fn compose(entry: &str) -> Result<String> {
    fn visit(path: &str, stack: &mut Vec<String>, done: &mut Vec<String>, out: &mut String) -> Result<()> {
        if done.iter().any(|d| d == path) {
            return Ok(());
        }
        if stack.iter().any(|s| s == path) {
            bail!("shader dependency cycle through {path}");
        }
        stack.push(path.to_string());
        let src = source(path)?;
        for dep in requires(src) {
            visit(dep, stack, done, out)?;
        }
        stack.pop();
        out.push_str(&format!("// ---- {path}\n"));
        out.push_str(src);
        if !src.ends_with('\n') {
            out.push('\n');
        }
        done.push(path.to_string());
        Ok(())
    }
    let mut out = String::new();
    visit(entry, &mut Vec::new(), &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// Server-only entry points (compiled by the worker with wgpu).
pub const SERVER_ENTRIES: &[&str] = &[
    "ingest/normalize.wgsl",
    "ingest/hilite_half.wgsl",
    "ingest/hilite_reduce.wgsl",
    "ingest/hilite_assemble.wgsl",
    "ingest/hilite_doub.wgsl",
    "ingest/rcd_conv.wgsl",
    "ingest/rcd_green.wgsl",
    "ingest/rcd_rb_diag.wgsl",
    "ingest/rcd_rb_green.wgsl",
    "ingest/xtrans_bin.wgsl",
    "ingest/downsample.wgsl",
    "ingest/resample.wgsl",
    "detail/to_opponent.wgsl",
    "detail/from_opponent.wgsl",
    "detail/atrous_blur.wgsl",
    "detail/shrink.wgsl",
    "detail/detail_band.wgsl",
    "detail/gauss.wgsl",
    "detail/rl_luma.wgsl",
    "detail/rl_ratio.wgsl",
    "detail/rl_update.wgsl",
    "detail/rl_apply.wgsl",
    "detail/crop.wgsl",
];

/// File name used for a composed entry, e.g. `client/develop.wgsl` -> `client_develop.wgsl`.
pub fn composed_name(entry: &str) -> String {
    entry.replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn develop_composes_each_module_once() {
        let s = compose("client/develop.wgsl").unwrap();
        assert_eq!(s.matches("// ---- common/color.wgsl").count(), 1);
        assert!(s.find("// ---- develop/params.wgsl").unwrap() < s.find("// ---- client/develop.wgsl").unwrap());
    }

    #[test]
    fn web_entries_parse_and_validate() {
        for entry in WEB_ENTRIES.iter().chain(SERVER_ENTRIES) {
            let src = compose(entry).unwrap();
            let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{entry}: {}", e.emit_to_string(&src)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{entry}: {}", e.emit_to_string(&src)));
        }
    }

    #[test]
    fn every_shader_has_an_spdx_header() {
        for (path, src) in FILES {
            assert!(src.starts_with("// SPDX-License-Identifier: "), "{path} lacks an SPDX header");
            let id = src.lines().next().unwrap();
            assert!(!id.contains("GPL"), "{path} carries a GPL-family licence");
            if id.contains("BSD-2-Clause") {
                assert!(src.contains("Ported from vkdt") || src.contains("vkdt src/pipe"), "{path} is BSD-2-Clause but doesn't name its origin");
            }
        }
    }
}
