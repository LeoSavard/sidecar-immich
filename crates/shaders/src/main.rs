// SPDX-License-Identifier: Apache-2.0
//! `sidecar-shaders <dir>`: compose the browser's WGSL entry points into `<dir>`. Lives here rather than in
//! sidecar-devtools so regenerating the shaders doesn't build LibRaw, wgpu and the rest of the worker.
use anyhow::{Result, bail};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        bail!("usage: sidecar-shaders <dir>")
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    for entry in sidecar_shaders::WEB_ENTRIES {
        let path = dir.join(sidecar_shaders::composed_name(entry));
        let composed = sidecar_shaders::compose(entry)?;
        // Leave an unchanged file alone, so Vite's watcher doesn't reload the editor for nothing.
        if std::fs::read_to_string(&path).ok().as_deref() != Some(composed.as_str()) {
            std::fs::write(&path, composed)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
