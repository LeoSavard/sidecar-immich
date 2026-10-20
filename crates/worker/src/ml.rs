// SPDX-License-Identifier: Apache-2.0
//! The worker's side of `sidecar-ml` (crates/ml): the program is started when a task needs it, talks over one
//! end of a Unix socket pair handed to it as stdin, and exits on its own after a few idle minutes, which is
//! how its GPU memory comes back. The next request starts it again.
//!
//! Present only in the `-ml` image: without the program or its weights, `Ml::discover` returns `None` and
//! every mask request answers "not available", so the plain image behaves as it always has.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// Where the `-ml` image puts the weights (Dockerfile, `ml` target).
const DEFAULT_MODELS: &str = "/usr/share/sidecar/models";
const WEIGHTS: [&str; 3] = ["yunet.bpk", "birefnet.bpk", "multiclass.bpk"];

/// An 8-bit sRGB picture, RGB interleaved: what the models read.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Per-pixel values at the picture's aspect ratio, `channels` interleaved, 0–255.
pub struct Planes {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub data: Vec<u8>,
}

/// One face; positions are fractions of the picture (mirrors `sidecar_ml::Face`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Face {
    pub score: f32,
    /// x, y, width, height.
    pub bounds: [f32; 4],
    /// The subject's right eye, left eye, nose tip, right and left mouth corners.
    pub landmarks: [[f32; 2]; 5],
}

/// The channels `parts` returns, in order (mirrors `sidecar_ml::PARTS`).
pub const PARTS: [&str; 6] = [
    "background",
    "hair",
    "body-skin",
    "face-skin",
    "clothes",
    "other",
];

pub struct Ml {
    program: PathBuf,
    models: PathBuf,
    cache: PathBuf,
    running: Mutex<Option<Running>>,
}

struct Running {
    child: Child,
    socket: UnixStream,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

impl Ml {
    /// The program from `SIDECAR_ML` or the `PATH`, the weights from `SIDECAR_ML_MODELS` or the image's
    /// default. `None` when either is missing. `cache` is where it keeps tuned kernels and compiled shaders.
    pub fn discover(cache: &Path) -> Option<Self> {
        let program = std::env::var_os("SIDECAR_ML")
            .map(PathBuf::from)
            .or_else(|| on_path("sidecar-ml"))?;
        let models = std::env::var_os("SIDECAR_ML_MODELS")
            .map_or_else(|| PathBuf::from(DEFAULT_MODELS), PathBuf::from);
        if !program.is_file() || !WEIGHTS.iter().all(|w| models.join(w).is_file()) {
            tracing::info!(
                "no machine-learning models here: masks are unavailable (the -ml image has them)"
            );
            return None;
        }
        tracing::info!(
            "masks from {} with the models in {}",
            program.display(),
            models.display()
        );
        Some(Self {
            program,
            models,
            cache: cache.to_path_buf(),
            running: Mutex::new(None),
        })
    }

    fn start(&self) -> Result<Running> {
        let (ours, theirs) = UnixStream::pair()?;
        let child = Command::new(&self.program)
            .arg("serve")
            .arg("--models")
            .arg(&self.models)
            .arg("--cache")
            .arg(&self.cache)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .spawn()
            .with_context(|| format!("starting {}", self.program.display()))?;
        Ok(Running {
            child,
            socket: ours,
        })
    }

    /// One request and its reply. A process that exited while idle, or died, is started again and the request
    /// sent once more: the first write or read to a gone process fails, so nothing is lost but the attempt.
    fn call(
        &self,
        header: serde_json::Value,
        pixels: &[u8],
    ) -> Result<(serde_json::Value, Vec<u8>)> {
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        for attempt in 0..2 {
            let alive = running
                .as_mut()
                .is_some_and(|r| matches!(r.child.try_wait(), Ok(None)));
            if !alive {
                *running = Some(self.start()?);
            }
            let r = running.as_mut().expect("just started");
            match exchange(&mut r.socket, &header, pixels) {
                Ok(reply) => return Ok(reply),
                Err(e) if attempt == 0 => {
                    tracing::info!("sidecar-ml went away ({e:#}); starting it again");
                    *running = None;
                }
                Err(e) => {
                    *running = None;
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    fn planes(&self, task: &str, picture: &Picture, long_edge: u32) -> Result<Planes> {
        let header = serde_json::json!({ "task": task, "width": picture.width, "height": picture.height, "longEdge": long_edge });
        let (reply, data) = self.call(header, &picture.rgb)?;
        let dim = |k: &str| {
            reply[k]
                .as_u64()
                .map(|v| v as u32)
                .with_context(|| format!("sidecar-ml sent no {k}"))
        };
        Ok(Planes {
            width: dim("width")?,
            height: dim("height")?,
            channels: dim("channels")?,
            data,
        })
    }

    /// The main subject, a soft mask at `long_edge` at most.
    pub fn subject(&self, picture: &Picture, long_edge: u32) -> Result<Planes> {
        self.planes("subject", picture, long_edge)
    }

    /// One channel per `PARTS` entry, summing to 255, at `long_edge` at most.
    pub fn parts(&self, picture: &Picture, long_edge: u32) -> Result<Planes> {
        self.planes("parts", picture, long_edge)
    }

    /// How near each point is, one channel, 255 the nearest, at `long_edge` at most.
    pub fn depth(&self, picture: &Picture, long_edge: u32) -> Result<Planes> {
        self.planes("depth", picture, long_edge)
    }

    pub fn faces(&self, picture: &Picture) -> Result<Vec<Face>> {
        let header = serde_json::json!({ "task": "faces", "width": picture.width, "height": picture.height });
        let (reply, _) = self.call(header, &picture.rgb)?;
        Ok(serde_json::from_value(reply["faces"].clone())?)
    }
}

/// Write one framed request, read one framed reply (the format is in crates/ml/src/main.rs).
fn exchange(
    socket: &mut UnixStream,
    header: &serde_json::Value,
    pixels: &[u8],
) -> Result<(serde_json::Value, Vec<u8>)> {
    let header = serde_json::to_vec(header)?;
    socket.write_all(&(header.len() as u32).to_le_bytes())?;
    socket.write_all(&header)?;
    socket.write_all(pixels)?;
    let mut len = [0u8; 4];
    socket.read_exact(&mut len)?;
    let mut reply = vec![0u8; u32::from_le_bytes(len) as usize];
    socket.read_exact(&mut reply)?;
    let reply: serde_json::Value = serde_json::from_slice(&reply)?;
    if reply["ok"] != true {
        bail!(
            "sidecar-ml: {}",
            reply["error"].as_str().unwrap_or("failed")
        );
    }
    let size: u64 = ["width", "height", "channels"]
        .iter()
        .map(|k| reply[*k].as_u64().unwrap_or(0))
        .product();
    ensure!(
        size <= 64 << 20,
        "sidecar-ml sent an implausible {size}-byte reply"
    );
    let mut payload = vec![0u8; size as usize];
    socket.read_exact(&mut payload)?;
    Ok((reply, payload))
}
