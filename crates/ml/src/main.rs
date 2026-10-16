// SPDX-License-Identifier: Apache-2.0
//! `sidecar-ml`: the models, as a program the worker starts when a task needs one.
//!
//!     sidecar-ml serve --models DIR --cache DIR [--idle 300]
//!     sidecar-ml run --models DIR faces|subject|parts|depth PICTURE [OUT.png]
//!
//! **serve** answers requests on stdin, which the worker makes one end of a Unix socket pair, so replies go back
//! on the same descriptor and stdout and stderr stay free for logs. It exits when stdin closes or after
//! `--idle` seconds without a request: burn keeps GPU memory until its process ends.
//!
//! A frame is a little-endian u32 length, that many bytes of JSON, then the pixels the JSON describes.
//!
//!     → {"task": "faces" | "subject" | "parts" | "depth", "width": W, "height": H, "longEdge": N}   W × H × 3 bytes, sRGB
//!     → {"task": "warm"}                                                                 no pixels
//!     ← {"ok": true, "faces": [...]}                                                     no pixels
//!     ← {"ok": true, "width": W, "height": H, "channels": C}                              W × H × C bytes
//!     ← {"ok": false, "error": "..."}                                                    no pixels
//!
//! `longEdge` bounds a mask's size (default 1024); masks are never larger than the picture.

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use sidecar_ml::{Face, Ml, PARTS, Picture, Planes, Precision};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Where the weights are (`dev/ml-models` writes them).
    #[arg(long, global = true, default_value = "/usr/share/sidecar/models")]
    models: PathBuf,
    /// Writable directory for what is learnt about this GPU: tuned kernels and the driver's shader cache.
    #[arg(long, global = true)]
    cache: Option<PathBuf>,
    /// f32 everywhere, even where the GPU has f16.
    #[arg(long, global = true)]
    f32: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        /// Seconds without a request before exiting.
        #[arg(long, default_value_t = 300)]
        idle: u64,
    },
    Run {
        task: Task,
        picture: PathBuf,
        /// Where to write a mask (PNG); faces go to stdout as JSON.
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1024)]
        long_edge: u32,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Task {
    Faces,
    Subject,
    Parts,
    Depth,
    Warm,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    task: Task,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default = "default_long_edge")]
    long_edge: u32,
}

fn default_long_edge() -> u32 {
    1024
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    faces: Option<Vec<Face>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<u32>,
}

enum Answer {
    Faces(Vec<Face>),
    Planes(Planes),
    Done,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(cache) = &cli.cache {
        keep_caches_in(cache)?;
    }
    // BiRefNet's forward pass is one very long generated function; the main thread's stack isn't enough.
    std::thread::Builder::new()
        .stack_size(sidecar_ml::STACK)
        .spawn(move || {
            let result = (|| {
                let precision = if cli.f32 { Precision::F32 } else { Precision::Auto };
                let mut ml = Ml::new(&cli.models, precision)?;
                eprintln!("sidecar-ml: GPU ready, {}", if ml.half() { "f16" } else { "f32" });
                match cli.command {
                    Command::Serve { idle } => serve(&mut ml, Duration::from_secs(idle)),
                    Command::Run { task, picture, out, long_edge } => run(&mut ml, task, &picture, out.as_deref(), long_edge),
                }
            })();
            leave(result)
        })?
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
    unreachable!("the model thread ends the process")
}

/// Ends the process without dropping the models or running exit handlers. At exit, the GPU driver's
/// handlers ran while burn's device threads were still alive, and about every other process then died with
/// glibc's "corrupted double-linked list" after its last reply had already gone out. Nothing is left that
/// the kernel doesn't reclaim.
fn leave(result: Result<()>) -> ! {
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    let _ = std::io::stdout().flush();
    // SAFETY: `_exit` only ends the process; stdout is flushed above and replies are written unbuffered.
    unsafe { libc::_exit(code) }
}

/// Tuned kernels and the GPU driver's compiled shaders both live under `dir`. Without the second, every start
/// recompiles every kernel: 30–90 s before the first mask instead of 1–6 s.
fn keep_caches_in(dir: &Path) -> Result<()> {
    let shaders = dir.join("shaders");
    std::fs::create_dir_all(&shaders).with_context(|| format!("creating {}", shaders.display()))?;
    sidecar_ml::keep_tuning_in(&dir.join("tuning"));
    // SAFETY: single-threaded here, before the GPU driver is loaded and reads them.
    unsafe {
        std::env::set_var("__GL_SHADER_DISK_CACHE_PATH", &shaders); // NVIDIA
        std::env::set_var("__GL_SHADER_DISK_CACHE_SIZE", "1073741824");
        std::env::set_var("MESA_SHADER_CACHE_DIR", &shaders); // AMD, Intel
    }
    Ok(())
}

fn answer(ml: &mut Ml, request: &Request, pixels: &[u8]) -> Result<Answer> {
    let picture = Picture { width: request.width, height: request.height, rgb: pixels };
    Ok(match request.task {
        Task::Faces => Answer::Faces(ml.faces(picture)?),
        Task::Subject => Answer::Planes(ml.subject(picture, request.long_edge)?),
        Task::Parts => Answer::Planes(ml.parts(picture, request.long_edge)?),
        Task::Depth => Answer::Planes(ml.depth(picture, request.long_edge)?),
        Task::Warm => {
            ml.warm()?;
            Answer::Done
        }
    })
}

fn serve(ml: &mut Ml, idle: Duration) -> Result<()> {
    let socket = UnixStream::from(std::io::stdin().as_fd().try_clone_to_owned()?);
    let mut replies = socket.try_clone()?;
    // Requests are read on their own thread so waiting for one can time out.
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut socket = socket;
        while let Ok(frame) = read_request(&mut socket) {
            if tx.send(frame).is_err() {
                break;
            }
        }
    });
    loop {
        let (request, pixels) = match rx.recv_timeout(idle) {
            Ok(Ok(frame)) => frame,
            Ok(Err(e)) => {
                write_frame(&mut replies, &Reply { error: Some(format!("{e:#}")), ..Reply::default() }, &[])?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                eprintln!("sidecar-ml: idle for {} s, exiting", idle.as_secs());
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        // A panic inside burn leaves the GPU in an unknown state: report it, then exit and let the worker
        // start a fresh process for the next request.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| answer(ml, &request, &pixels)));
        let (reply, payload) = match result {
            Ok(Ok(Answer::Faces(faces))) => (Reply { ok: true, faces: Some(faces), ..Reply::default() }, Vec::new()),
            Ok(Ok(Answer::Planes(p))) => {
                (Reply { ok: true, width: Some(p.width), height: Some(p.height), channels: Some(p.channels), ..Reply::default() }, p.data)
            }
            Ok(Ok(Answer::Done)) => (Reply { ok: true, ..Reply::default() }, Vec::new()),
            Ok(Err(e)) => (Reply { error: Some(format!("{e:#}")), ..Reply::default() }, Vec::new()),
            Err(_) => {
                write_frame(&mut replies, &Reply { error: Some("the model crashed".into()), ..Reply::default() }, &[])?;
                bail!("a model panicked");
            }
        };
        write_frame(&mut replies, &reply, &payload)?;
    }
}

/// Largest picture accepted: a 64 MP photo. Requests are expected at proxy size, far below it.
const MAX_PIXELS: u64 = 64 << 20;

/// One request, or an error naming what was wrong with it. An `Err` from reading the stream itself ends the loop.
fn read_request(socket: &mut UnixStream) -> std::io::Result<Result<(Request, Vec<u8>)>> {
    let mut len = [0u8; 4];
    socket.read_exact(&mut len)?;
    let mut header = vec![0u8; u32::from_le_bytes(len) as usize];
    socket.read_exact(&mut header)?;
    let request: Request = match serde_json::from_slice(&header) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e).context("reading a request")),
    };
    let size = request.width as u64 * request.height as u64;
    if !matches!(request.task, Task::Warm) && (size == 0 || size > MAX_PIXELS) {
        // The pixels can't be skipped without trusting the size, so the stream is out of step: end it.
        return Err(std::io::Error::other(format!("a {} × {} picture", request.width, request.height)));
    }
    let mut pixels = vec![0u8; if matches!(request.task, Task::Warm) { 0 } else { size as usize * 3 }];
    socket.read_exact(&mut pixels)?;
    Ok(Ok((request, pixels)))
}

fn write_frame(socket: &mut UnixStream, reply: &Reply, payload: &[u8]) -> Result<()> {
    let header = serde_json::to_vec(reply)?;
    socket.write_all(&(header.len() as u32).to_le_bytes())?;
    socket.write_all(&header)?;
    socket.write_all(payload)?;
    Ok(())
}

fn run(ml: &mut Ml, task: Task, path: &Path, out: Option<&Path>, long_edge: u32) -> Result<()> {
    let (width, height, rgb) = decode(path)?;
    let request = Request { task, width, height, long_edge };
    let started = std::time::Instant::now();
    let answer = answer(ml, &request, &rgb)?;
    eprintln!("sidecar-ml: {task:?} in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
    match answer {
        Answer::Faces(faces) => println!("{}", serde_json::to_string_pretty(&faces)?),
        Answer::Planes(planes) => {
            let out = out.context("give a path for the mask")?;
            write_png(out, &planes)?;
        }
        Answer::Done => {}
    }
    Ok(())
}

/// Parts as colours (background black, hair brown, body skin tan, face skin pink, clothes blue, other green);
/// a subject or depth mask as grey.
fn write_png(path: &Path, planes: &Planes) -> Result<()> {
    const COLOURS: [[f32; 3]; 6] = [[0.0, 0.0, 0.0], [200.0, 120.0, 40.0], [240.0, 200.0, 170.0], [255.0, 150.0, 150.0], [60.0, 90.0, 220.0], [120.0, 220.0, 120.0]];
    let (colour, data) = if planes.channels as usize == PARTS.len() {
        let rgb = planes
            .data
            .as_chunks::<{ PARTS.len() }>()
            .0
            .iter()
            .flat_map(|p| {
                let mix = |c: usize| p.iter().zip(&COLOURS).map(|(w, col)| *w as f32 / 255.0 * col[c]).sum::<f32>().round() as u8;
                [mix(0), mix(1), mix(2)]
            })
            .collect();
        (png::ColorType::Rgb, rgb)
    } else {
        (png::ColorType::Grayscale, planes.data.clone())
    };
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), planes.width, planes.height);
    encoder.set_color(colour);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}

/// A PNG or JPEG as 8-bit RGB.
fn decode(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.starts_with(b"\x89PNG") {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().context("PNG too large")?];
        let info = reader.next_frame(&mut buf)?;
        let channels = info.color_type.samples();
        let rgb = buf[..info.buffer_size()]
            .chunks_exact(channels)
            .flat_map(|p| if channels < 3 { [p[0]; 3] } else { [p[0], p[1], p[2]] })
            .collect();
        return Ok((info.width, info.height, rgb));
    }
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(bytes));
    let pixels = decoder.decode().context("decoding the JPEG")?;
    let info = decoder.info().context("JPEG has no frame header")?;
    let channels = decoder.output_colorspace().map(|c| c.num_components()).unwrap_or(3);
    let (w, h) = (info.width as usize, info.height as usize);
    ensure!(pixels.len() == w * h * channels, "unexpected JPEG layout");
    let rgb = pixels.chunks_exact(channels).flat_map(|p| if channels < 3 { [p[0]; 3] } else { [p[0], p[1], p[2]] }).collect();
    Ok((w as u32, h as u32, rgb))
}
