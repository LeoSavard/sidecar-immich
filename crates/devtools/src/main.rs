// SPDX-License-Identifier: Apache-2.0
//! `sidecar-dev parity fixtures <dir>`    write synthetic sources + packed uniforms for each parity case
//! `sidecar-dev parity native <dir> <out>`  render every case with wgpu (Vulkan on the server)
//! `sidecar-dev parity compare <out>`     compare <case>.native.f16 with <case>.browser.f16 (ΔE2000)
//! `sidecar-dev develop <raw> <out.png> [long_edge]`  decode → proxy → GPU develop (as shot) → PNG
//! `sidecar-dev png <in.f16> <width> <height> <out.png>`  sRGB-encode a display-linear RGBA16F dump for viewing
//! `sidecar-dev hilite <raw> <dir> [--crop]`  chromaticity by clip coverage: what reconstruction leaves
//! `sidecar-dev hilite-truth <raw> <ev>…`  reconstruction against ground truth: an unclipped frame, clipped by
//!                                        raising its mosaic `ev` stops, compared with itself
//! `sidecar-dev lens <file>…`             the lens correction and identity each file yields
//! `sidecar-dev radial <list>`            measured radial falloff per RAW (vignetting checks)
//! `sidecar-dev camera-jpeg <list>`       a RAW's lens profile checked against the camera's own JPEG

mod parity;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["ingest", input, out, rest @ ..] => ingest(
            input.as_ref(),
            out.as_ref(),
            rest.first().map_or(Ok(1600), |v| v.parse())?,
        ),
        ["develop", input, out, rest @ ..] => develop(
            input.as_ref(),
            out.as_ref(),
            rest.first().map_or(Ok(1600), |v| v.parse())?,
        ),
        ["png", input, w, h, out] => {
            let img = sidecar_engine::RgbaF32::from_f16_bytes(
                w.parse()?,
                h.parse()?,
                &std::fs::read(input)?,
            );
            write_png(&img, out.as_ref())
        }
        ["hilite", input, out_dir, rest @ ..] => {
            hilite(input.as_ref(), out_dir.as_ref(), rest.contains(&"--crop"))
        }
        ["hilite-truth", input, evs @ ..] if !evs.is_empty() => hilite_truth(
            input.as_ref(),
            &evs.iter()
                .map(|v| v.parse())
                .collect::<Result<Vec<f32>, _>>()?,
        ),
        ["radial", list] => radial(list.as_ref()),
        ["levels", input] => levels(input.as_ref()),
        ["linear-dng", input, out, rest @ ..] => {
            let input: &std::path::Path = input.as_ref();
            let edge: u32 = rest.first().map_or(Ok(1600), |v| v.parse())?;
            let t0 = std::time::Instant::now();
            let info = sidecar_raw::linear_dng::info(input, &sidecar_raw::decoded::exif(input))?;
            let img = sidecar_raw::linear_dng::decode(input, edge)?;
            let t_decode = t0.elapsed();
            let src = sidecar_engine::RgbaF32 {
                width: img.width,
                height: img.height,
                data: img.rgba,
            }
            .oriented(info.orientation_flip);
            let gpu = sidecar_engine::Gpu::new_headless()?;
            let pass = sidecar_engine::DevelopPass::new(&gpu)?;
            let edit: sidecar_color::EditParams = match std::env::var("SIDECAR_EDIT") {
                Ok(json) => serde_json::from_str(&json)?,
                Err(_) => Default::default(),
            };
            let packed = sidecar_color::pack(
                &edit,
                &info.color,
                info.source_kind,
                sidecar_color::DisplaySpace::Srgb,
            );
            let out_img = pass.run(&gpu, &src, &packed.uniform, &packed.lut)?;
            write_png(&out_img, out.as_ref())?;
            println!(
                "{} {} · {}×{} → {}×{} · as shot {:.0} K {:+.0} · decode {:.1} s",
                info.make,
                info.model,
                info.width,
                info.height,
                out_img.width,
                out_img.height,
                packed.temperature_k,
                packed.tint,
                t_decode.as_secs_f64()
            );
            Ok(())
        }
        ["lens", files @ ..] => {
            for f in files {
                let mut info = match sidecar_raw::RawFile::metadata(f.as_ref()) {
                    Ok(i) => i,
                    Err(e) => {
                        println!("{f}\n  {e}");
                        continue;
                    }
                };
                sidecar_raw::read_extras(f.as_ref(), &mut info);
                let p = info.lens_profile.as_ref();
                let has = |c: Option<&sidecar_color::lens::Curve>, r: f64| {
                    c.map_or("-".to_string(), |c| format!("{:.4}", c.eval(r)))
                };
                println!(
                    "{f}\n  {} {} · lens {:?} · profile {} · distortion@1 {} · red@1 {} · blue@1 {} · vignetting@1 {}{}",
                    info.make,
                    info.model,
                    info.lens_identity.as_ref().and_then(|i| i.lens_id.clone()),
                    p.map_or("none", |p| p.id.as_str()),
                    has(p.and_then(|p| p.distortion.as_ref()), 1.0),
                    has(p.and_then(|p| p.ca_red.as_ref()), 1.0),
                    has(p.and_then(|p| p.ca_blue.as_ref()), 1.0),
                    has(p.and_then(|p| p.vignetting.as_ref()), 1.0),
                    if p.is_some_and(|p| p.required) {
                        " · required"
                    } else {
                        ""
                    },
                );
            }
            Ok(())
        }
        ["camera-jpeg", list] => camera_jpeg(list.as_ref()),
        ["parity", "fixtures", dir] => parity::fixtures(dir.as_ref()),
        ["parity", "native", dir, out] => parity::native(dir.as_ref(), out.as_ref()),
        ["parity", "compare", dir, out] => parity::compare(dir.as_ref(), out.as_ref()),
        _ => bail!(
            "usage: sidecar-dev parity fixtures <dir> | parity native <dir> <out> | parity compare <dir> <out>"
        ),
    }
}

fn write_png(img: &sidecar_engine::RgbaF32, out: &std::path::Path) -> Result<()> {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0 + 0.5) as u8
    };
    let rgb: Vec<u8> = img
        .data
        .chunks(4)
        .flat_map(|p| [enc(p[0]), enc(p[1]), enc(p[2])])
        .collect();
    let mut e = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(out)?),
        img.width,
        img.height,
    );
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn develop(input: &std::path::Path, out: &std::path::Path, long_edge: u32) -> Result<()> {
    use std::time::Instant;
    let t0 = Instant::now();
    let mut raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let t_open = t0.elapsed();
    // Half-size demosaic unless more than half the sensor is asked for.
    let linear = raw.develop_linear(long_edge * 2 <= info.width.max(info.height))?;
    let t_demosaic = t0.elapsed();
    let proxy = sidecar_engine::RgbaF32 {
        width: linear.width,
        height: linear.height,
        data: linear.rgba(),
    }
    .downsampled(long_edge);
    let t_proxy = t0.elapsed();
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let pass = sidecar_engine::DevelopPass::new(&gpu)?;
    let edit: sidecar_color::EditParams = match std::env::var("SIDECAR_EDIT") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => Default::default(),
    };
    let packed = sidecar_color::pack(
        &edit,
        &info.color,
        info.source_kind,
        sidecar_color::DisplaySpace::Srgb,
    );
    let src = proxy;
    let t_gpu0 = std::time::Instant::now();
    let masks = sidecar_engine::Masks {
        mono: &packed.mono_lut,
        ..sidecar_engine::Masks::none()
    };
    let developed = pass.render(
        &gpu,
        &sidecar_engine::upload_rgba16(&gpu, &src),
        &packed.uniform,
        &packed.lut,
        &masks,
    )?;
    let img = sidecar_engine::read_rgba16(&gpu, &developed)?;
    let t_gpu = t_gpu0.elapsed();
    write_png(&img, out)?;
    println!(
        "{} {} ({:?}) {}×{} → proxy {}×{} · as shot {:.0} K {:+.0} · open {:.0} ms, half-size demosaic {:.0} ms, proxy {:.0} ms, GPU develop {:.0} ms",
        info.make,
        info.model,
        info.sensor,
        info.width,
        info.height,
        img.width,
        img.height,
        packed.temperature_k,
        packed.tint,
        t_open.as_secs_f64() * 1e3,
        (t_demosaic - t_open).as_secs_f64() * 1e3,
        (t_proxy - t_demosaic).as_secs_f64() * 1e3,
        t_gpu.as_secs_f64() * 1e3
    );
    Ok(())
}

/// What highlight reconstruction leaves behind, measured rather than squinted at: the proxy's alpha is the
/// fraction of source pixels that clipped under each output pixel, so bucketing by it sorts untouched picture
/// (0) from the heart of a blown area (1) with the reconstructed rim in between. For each bucket this prints
/// the mean chromaticity in white-balanced camera RGB — `r/(r+g+b)` and `b/(r+g+b)`, which a neutral renders
/// at 0.333 each — so a cast shows up as a number instead of an impression, and its sign says which way.
/// `--crop` writes the worst region magnified, for looking at.
/// Highlight reconstruction measured against the truth. A frame that clips nowhere is clipped on purpose: its
/// mosaic is raised `ev` stops and cut at the sensor's white, which saturates each colour where a longer
/// exposure would have, green and red and blue at their own scene luminance. The proxy of that is compared,
/// pixel for pixel, with the unclipped proxy raised by the same `ev`, which is what reconstruction should have
/// recovered. Reported by how much of each pixel clipped: the colour error (white-balanced chromaticity, and
/// its signed bias, which is the cast) and the brightness ratio in stops. `SIDECAR_NO_HILITE=1` gives the
/// clipped baseline to beat.
fn hilite_truth(input: &std::path::Path, evs: &[f32]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ing = sidecar_engine::Ingest::new(&gpu)?;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    let mul = info.color.as_shot_mul;
    let wb = sidecar_color::camera::resolve(&info.color, info.color.as_shot_xy());
    let run = |data: &[u16]| {
        let input_m = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        ing.proxy(&gpu, &input_m, [mul[0] as f32, 1.0, mul[2] as f32], 1600)
    };
    let truth = run(&m.data)?;
    let clipped_already = truth.data.chunks(4).filter(|p| p[3] > 0.0).count();
    println!(
        "{} {} · proxy {}×{} · {:.3} % of the unraised proxy already touches clipping",
        info.make,
        info.model,
        truth.width,
        truth.height,
        100.0 * clipped_already as f64 / (truth.data.len() / 4) as f64
    );
    let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
    const EDGES: [f32; 4] = [0.0, 0.5, 0.9, 1.01];
    for &ev in evs {
        let k = 2f32.powf(ev);
        let raised: Vec<u16> = m
            .data
            .iter()
            .map(|&v| (blk + (v as f32 - blk).max(0.0) * k).min(m.white) as u16)
            .collect();
        let got = run(&raised)?;
        let (mut n, mut err, mut blended, mut bias, mut stops, mut bad) = (
            [0u64; 4],
            [0f64; 4],
            [0f64; 4],
            [[0f64; 2]; 4],
            [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            [0u64; 4],
        );
        for (t, g) in truth.data.chunks(4).zip(got.data.chunks(4)) {
            if t[3] > 0.0 {
                continue;
            }
            let bal = |p: &[f32], s: f64| {
                [
                    p[0] as f64 * wb.mul[0] * s,
                    p[1] as f64 * s,
                    p[2] as f64 * wb.mul[2] * s,
                ]
            };
            let (tc, gc) = (bal(t, k as f64), bal(g, 1.0));
            let (ts, gs) = (tc.iter().sum::<f64>(), gc.iter().sum::<f64>());
            if ts <= 1e-6 {
                continue;
            }
            let b = if g[3] <= 0.0 {
                0
            } else {
                EDGES
                    .iter()
                    .position(|&e| g[3] < e)
                    .unwrap_or(4)
                    .clamp(1, 3)
            };
            n[b] += 1;
            if !gs.is_finite() || gs <= 1e-9 {
                bad[b] += 1;
                continue;
            }
            let d = [gc[0] / gs - tc[0] / ts, gc[2] / gs - tc[2] / ts];
            err[b] += (d[0] * d[0] + d[1] * d[1]).sqrt();
            // The develop pass's highlight blend at its default (`exposure.wgsl`), toward the mean: the same
            // sum, so the chromaticity moves toward a third each by alpha × 0.5.
            let t = (g[3] as f64 * 0.5).clamp(0.0, 1.0);
            let e = [
                gc[0] / gs + (1.0 / 3.0 - gc[0] / gs) * t - tc[0] / ts,
                gc[2] / gs + (1.0 / 3.0 - gc[2] / gs) * t - tc[2] / ts,
            ];
            blended[b] += (e[0] * e[0] + e[1] * e[1]).sqrt();
            bias[b][0] += d[0];
            bias[b][1] += d[1];
            stops[b].push((gs / ts).log2());
        }
        println!(
            "  +{ev} EV       pixels   colour error (after blend)   bias r     bias b    stops vs truth (p10 / median / p90)   non-finite"
        );
        let names = ["unclipped", "clip < 0.5", "0.5–0.9", "≥ 0.9"];
        for b in 0..4 {
            if n[b] == 0 {
                continue;
            }
            let k = (n[b] - bad[b]).max(1) as f64;
            let s = &mut stops[b];
            s.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f64| {
                s.get(((s.len() as f64 - 1.0) * f) as usize)
                    .copied()
                    .unwrap_or(f64::NAN)
            };
            println!(
                "  {:<11} {:>9}   {:>10.4}   ({:.4})       {:>+8.4}   {:>+8.4}   {:>+7.2} / {:>+7.2} / {:>+7.2}            {}",
                names[b],
                n[b],
                err[b] / k,
                blended[b] / k,
                bias[b][0] / k,
                bias[b][1] / k,
                q(0.1),
                q(0.5),
                q(0.9),
                bad[b]
            );
        }
    }
    Ok(())
}

fn hilite(input: &std::path::Path, out_dir: &std::path::Path, crop: bool) -> Result<()> {
    const BUCKETS: usize = 10;
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ing = sidecar_engine::Ingest::new(&gpu)?;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    let mul = info.color.as_shot_mul;
    let input_m = sidecar_engine::MosaicInput {
        width: m.width,
        height: m.height,
        data: &m.data,
        black: m.black,
        white: m.white,
        pattern: m.pattern,
        period: m.period,
    };
    let proxy = ing
        .proxy(&gpu, &input_m, [mul[0] as f32, 1.0, mul[2] as f32], 1600)?
        .oriented(m.flip);
    let wb = sidecar_color::camera::resolve(&info.color, info.color.as_shot_xy());

    // Which channels actually reach saturation, counted on the mosaic itself. This is what decides whether a
    // blown area still holds a usable measurement: where only one colour has clipped, the other two are real
    // and reconstruction has something honest to hold on to; where all three have gone, it is inventing.
    {
        let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
        let white_obs =
            ((m.data.iter().copied().max().unwrap_or(0) as f32 - blk) / (m.white - blk)).min(1.0);
        let clip = if white_obs > 0.5 {
            (white_obs * 0.98).min(0.985)
        } else {
            0.985
        };
        let mut hit = [0u64; 3];
        let mut seen = [0u64; 3];
        for (i, &v) in m.data.iter().enumerate() {
            let (x, y) = ((i as u32) % m.width, (i as u32) / m.width);
            let c = match m.pattern[(y % m.period) as usize][(x % m.period) as usize] {
                3 => 1,
                c => c,
            } as usize;
            let n = (v as f32 - m.black[c]) / (m.white - m.black[c]);
            seen[c] += 1;
            if n >= clip {
                hit[c] += 1;
            }
        }
        let pct = |c: usize| 100.0 * hit[c] as f64 / seen[c].max(1) as f64;
        println!(
            "  mosaic at clip {clip:.3}: R {:.3} %, G {:.3} %, B {:.3} %",
            pct(0),
            pct(1),
            pct(2)
        );
    }

    let (mut sum, mut n) = ([[0f64; 3]; BUCKETS], [0u64; BUCKETS]);
    for p in proxy.data.chunks(4) {
        let c = [
            p[0] as f64 * wb.mul[0],
            p[1] as f64,
            p[2] as f64 * wb.mul[2],
        ];
        let t = c[0] + c[1] + c[2];
        if t <= 1e-6 {
            continue;
        }
        let b = ((p[3].clamp(0.0, 1.0) * BUCKETS as f32) as usize).min(BUCKETS - 1);
        for i in 0..3 {
            sum[b][i] += c[i] / t;
        }
        n[b] += 1;
    }
    println!(
        "{} {} · proxy {}×{} · as shot {:.0} K {:+.0}",
        info.make, info.model, proxy.width, proxy.height, wb.temperature_k, wb.tint
    );
    println!("  clip coverage   pixels      r/(r+g+b)   b/(r+g+b)");
    for b in 0..BUCKETS {
        if n[b] == 0 {
            continue;
        }
        let (lo, hi) = (b as f64 / BUCKETS as f64, (b + 1) as f64 / BUCKETS as f64);
        let k = n[b] as f64;
        println!(
            "  {lo:.1}–{hi:.1}  {:>12}      {:.4}      {:.4}",
            n[b],
            sum[b][0] / k,
            sum[b][2] / k
        );
    }

    if crop {
        // The densest window of partly-clipped pixels: where a rim artifact would live.
        let (w, h) = (proxy.width as usize, proxy.height as usize);
        let (side, step) = (160usize, 16usize);
        let (mut best, mut at) = (0f64, (0usize, 0usize));
        let mut y = 0;
        while y + side <= h {
            let mut x = 0;
            while x + side <= w {
                let mut score = 0.0;
                for yy in (y..y + side).step_by(4) {
                    for xx in (x..x + side).step_by(4) {
                        let a = proxy.data[(yy * w + xx) * 4 + 3];
                        if (0.05..0.95).contains(&a) {
                            score += 1.0;
                        }
                    }
                }
                if score > best {
                    (best, at) = (score, (x, y));
                }
                x += step;
            }
            y += step;
        }
        let pass = sidecar_engine::DevelopPass::new(&gpu)?;
        let packed = sidecar_color::pack(
            &Default::default(),
            &info.color,
            info.source_kind,
            sidecar_color::DisplaySpace::Srgb,
        );
        let masks = sidecar_engine::Masks {
            mono: &packed.mono_lut,
            ..sidecar_engine::Masks::none()
        };
        let dev = sidecar_engine::read_rgba16(
            &gpu,
            &pass.render(
                &gpu,
                &sidecar_engine::upload_rgba16(&gpu, &proxy),
                &packed.uniform,
                &packed.lut,
                &masks,
            )?,
        )?;
        const Z: usize = 4;
        let mut cut = sidecar_engine::RgbaF32 {
            width: (side * Z) as u32,
            height: (side * Z) as u32,
            data: vec![0.0; side * Z * side * Z * 4],
        };
        for yy in 0..side * Z {
            for xx in 0..side * Z {
                let src = ((at.1 + yy / Z) * w + at.0 + xx / Z) * 4;
                let dst = (yy * side * Z + xx) * 4;
                cut.data[dst..dst + 4].copy_from_slice(&dev.data[src..src + 4]);
            }
        }
        std::fs::create_dir_all(out_dir)?;
        let stem = input
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        write_png(&cut, &out_dir.join(format!("{stem}-rim.png")))?;
        println!(
            "  worst rim at {},{} ({} partly-clipped samples) → {stem}-rim.png",
            at.0, at.1, best as u64
        );
    }
    Ok(())
}

/// Measured vignetting: for each RAW listed in `list` (one path per line), the mean log2 of the green channel
/// in 20 rings of radius (1 = half-diagonal), relative to the centre ring, as one JSON line. Averaged over
/// many photos, the scene term evens out and what is left is the lens falloff — the check on how a maker's
/// vignetting table is scaled, where no second opinion from Adobe exists.
fn radial(list: &std::path::Path) -> Result<()> {
    const RINGS: usize = 20;
    for path in std::fs::read_to_string(list)?
        .lines()
        .filter(|l| !l.trim().is_empty())
    {
        let Ok(mut raw) = sidecar_raw::RawFile::open(path.as_ref()) else {
            continue;
        };
        let Ok(img) = raw.develop_linear(true) else {
            continue;
        };
        let (w, h) = (img.width as f64, img.height as f64);
        let hd = (w * w + h * h).sqrt() / 2.0;
        let (mut sum, mut n) = ([0f64; RINGS], [0u64; RINGS]);
        for (i, p) in img.rgb.chunks(3).enumerate() {
            let g = p[1];
            if !(256..60000).contains(&g) {
                continue;
            }
            let (x, y) = (
                (i as f64 % w) + 0.5 - w / 2.0,
                (i as f64 / w).floor() + 0.5 - h / 2.0,
            );
            let ring =
                (((x * x + y * y).sqrt() / hd) * RINGS as f64).min(RINGS as f64 - 1.0) as usize;
            sum[ring] += (g as f64).log2();
            n[ring] += 1;
        }
        if n[0] < 100 {
            continue;
        }
        let centre = sum[0] / n[0] as f64;
        let profile: Vec<Option<f64>> = (0..RINGS)
            .map(|r| (n[r] > 100).then(|| sum[r] / n[r] as f64 - centre))
            .collect();
        println!(
            "{}",
            serde_json::json!({ "path": path, "profile": profile })
        );
    }
    Ok(())
}

/// The camera's own rendering as the reference for its lens tables. For each RAW in `list`, the embedded
/// JPEG — which the camera corrected for distortion and shading itself — is aligned to the RAW through the
/// file's distortion profile, and the gain the camera applied is read ring by ring: the camera's tone curve is
/// learned from the centre of the frame (where shading is nil) and inverted everywhere else. Scene content
/// cancels, because each JPEG pixel is compared with the same point of the same exposure.
///
/// One JSON line per file: the alignment score (normalized cross-correlation of log brightness) with the
/// profile and without it, the zoom that aligns best, and the measured gain per ring, in stops.
fn camera_jpeg(list: &std::path::Path) -> Result<()> {
    const RINGS: usize = 20;
    // With SIDECAR_LENSFUN set to a directory of lensfun XML, check lensfun's profile instead of the file's.
    let lensfun = std::env::var("SIDECAR_LENSFUN")
        .ok()
        .map(|d| sidecar_raw::lensfun::Database::load(d.as_ref()));
    for path in std::fs::read_to_string(list)?
        .lines()
        .filter(|l| !l.trim().is_empty())
    {
        let out = std::process::Command::new("exiftool")
            .arg("-j")
            .args(sidecar_raw::lens::TAGS)
            .arg(path)
            .output()?;
        let Some(tags) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .ok()
            .and_then(|v| v.get(0).cloned())
        else {
            continue;
        };
        // The largest rendering the camera embedded: Nikon's full-size JpgFromRaw, else the preview.
        let mut preview = std::process::Command::new("exiftool")
            .args(["-b", "-JpgFromRaw"])
            .arg(path)
            .output()?
            .stdout;
        if preview.len() < 1000 {
            preview = std::process::Command::new("exiftool")
                .args(["-b", "-PreviewImage"])
                .arg(path)
                .output()?
                .stdout;
        }
        let Ok(mut raw) = sidecar_raw::RawFile::open(path.as_ref()) else {
            continue;
        };
        let mut info = raw.info();
        let size = if info.orientation_flip & 4 != 0 {
            [info.height, info.width]
        } else {
            [info.width, info.height]
        };
        let profile = match &lensfun {
            Some(db) => {
                sidecar_raw::read_extras(path.as_ref(), &mut info);
                db.profile(
                    &info,
                    info.lens_identity.as_ref().unwrap_or(&Default::default()),
                )
            }
            None => sidecar_raw::lens::from_tags(&tags, size),
        };
        let Some(profile) = profile else { continue };
        let Some(dist) = profile.distortion.clone() else {
            continue;
        };
        let Ok(img) = raw.develop_linear(true) else {
            continue;
        };
        let Ok(jpeg) = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview)).decode() else {
            continue;
        };
        let Some((jw, jh)) = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview))
            .dimensions()
            .or_else(|| {
                let mut d = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview));
                d.decode_headers().ok()?;
                d.dimensions()
            })
        else {
            continue;
        };
        if jpeg.len() != jw * jh * 3 {
            continue;
        }
        // develop_linear applies the orientation; the preview is stored unrotated. Compare in the RAW's frame
        // only when both agree on landscape.
        let (rw, rh) = (img.width as usize, img.height as usize);
        if (rw > rh) != (jw > jh) {
            continue;
        }
        let lin = |v: u8| {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        let jhd = ((jw * jw + jh * jh) as f64).sqrt() / 2.0;
        // About 200,000 samples whatever the size: a full-size 24 MP JpgFromRaw at every other pixel is 30× that.
        let step = ((jw * jh) as f64 / 200_000.0).sqrt().ceil().max(1.0) as usize;
        let rhd = ((rw * rw + rh * rh) as f64).sqrt() / 2.0;
        // Sample JPEG pixel → (radius in half-diagonals, log2 JPEG linear, log2 RAW green), for a distortion
        // curve and zoom.
        let samples = |curve: &dyn Fn(f64) -> f64, zoom: f64| -> Vec<(f64, f64, f64)> {
            let mut v = Vec::new();
            for y in (0..jh).step_by(step) {
                for x in (0..jw).step_by(step) {
                    let u = [
                        ((x as f64 + 0.5) - jw as f64 / 2.0) / jhd * zoom,
                        ((y as f64 + 0.5) - jh as f64 / 2.0) / jhd * zoom,
                    ];
                    let r = (u[0] * u[0] + u[1] * u[1]).sqrt();
                    let s = curve(r);
                    let (sx, sy) = (
                        rw as f64 / 2.0 + u[0] * s * rhd,
                        rh as f64 / 2.0 + u[1] * s * rhd,
                    );
                    if sx < 0.0 || sy < 0.0 || sx >= rw as f64 - 1.0 || sy >= rh as f64 - 1.0 {
                        continue;
                    }
                    let g = img.rgb[(sy as usize * rw + sx as usize) * 3 + 1];
                    let j = jpeg[(y * jw + x) * 3 + 1];
                    if !(64..60000).contains(&g) || !(8..=245).contains(&j) {
                        continue;
                    }
                    v.push((r, lin(j).log2(), (g as f64).log2()));
                }
            }
            v
        };
        let ncc = |s: &[(f64, f64, f64)]| {
            let n = s.len() as f64;
            let (ma, mb) = (
                s.iter().map(|p| p.1).sum::<f64>() / n,
                s.iter().map(|p| p.2).sum::<f64>() / n,
            );
            let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
            for p in s {
                ab += (p.1 - ma) * (p.2 - mb);
                aa += (p.1 - ma).powi(2);
                bb += (p.2 - mb).powi(2);
            }
            ab / (aa * bb).sqrt()
        };
        let best = |curve: &dyn Fn(f64) -> f64| -> (f64, f64) {
            (0..=40)
                .map(|i| 0.9 + i as f64 * 0.005)
                .map(|z| (z, ncc(&samples(curve, z))))
                .fold((1.0, -1.0), |a, b| if b.1 > a.1 { b } else { a })
        };
        let (zoom, with) = best(&|r| dist.eval(r));
        let (_, without) = best(&|_| 1.0);
        // How far off the profile's radius scale is: the best-aligning stretch of its radius axis (1 = as read).
        let stretch = (0..=16)
            .map(|i| 0.84 + i as f64 * 0.02)
            .map(|k| (k, best(&|r| dist.eval(r * k)).1))
            .fold((1.0, -1.0), |a, b| if b.1 > a.1 { b } else { a })
            .0;
        // The camera's tone curve from the centre: median JPEG value per RAW-brightness bin.
        let s = samples(&|r| dist.eval(r), zoom);
        let lo = s.iter().map(|p| p.2).fold(f64::MAX, f64::min);
        let hi = s.iter().map(|p| p.2).fold(f64::MIN, f64::max);
        let bins = 48;
        let bin = |g: f64| (((g - lo) / (hi - lo + 1e-9)) * bins as f64) as usize;
        let mut centre: Vec<Vec<f64>> = vec![Vec::new(); bins + 1];
        for p in s.iter().filter(|p| p.0 < 0.3) {
            centre[bin(p.2)].push(p.1);
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied()
        };
        // (RAW log2, JPEG log2) points, made monotone.
        let mut curve: Vec<(f64, f64)> = centre
            .iter_mut()
            .enumerate()
            .filter(|(_, v)| v.len() >= 30)
            .filter_map(|(i, v)| {
                Some((lo + (i as f64 + 0.5) / bins as f64 * (hi - lo), median(v)?))
            })
            .collect();
        for i in 1..curve.len() {
            curve[i].1 = curve[i].1.max(curve[i - 1].1);
        }
        if curve.len() < 8 {
            continue;
        }
        // JPEG log2 → the RAW log2 the centre would have needed to render it.
        let invert = |j: f64| -> Option<f64> {
            let k = curve
                .windows(2)
                .position(|w| j >= w[0].1 && j <= w[1].1 && w[1].1 > w[0].1)?;
            let (a, b) = (curve[k], curve[k + 1]);
            Some(a.0 + (j - a.1) / (b.1 - a.1) * (b.0 - a.0))
        };
        let mut rings: Vec<Vec<f64>> = vec![Vec::new(); RINGS];
        for p in &s {
            if let Some(g) = invert(p.1) {
                rings[((p.0 * RINGS as f64) as usize).min(RINGS - 1)].push(g - p.2);
            }
        }
        let gain: Vec<Option<f64>> = rings
            .iter_mut()
            .map(|v| if v.len() >= 50 { median(v) } else { None })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "path": path, "profile": profile.id, "ncc": with, "nccWithout": without, "zoom": zoom, "stretch": stretch, "gainEv": gain })
        );
    }
    Ok(())
}

/// Where each CFA colour actually saturates, by the histogram of the top of its range, beside the single
/// brightest sample the old heuristics keyed off (D6/D7 in docs/research/pipeline-review-2026-10.md). On a
/// frame with blown highlights the two should agree; where they don't, the histogram is the one that
/// survives a stuck pixel.
fn levels(input: &std::path::Path) -> Result<()> {
    use sidecar_raw::levels::TopOfRange;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    println!(
        "{} {} \u{b7} {}\u{d7}{} \u{b7} declared maximum {} \u{b7} black {:?}",
        info.make, info.model, m.width, m.height, m.white, m.black
    );
    let mut tops: [TopOfRange; 4] = std::array::from_fn(|c| TopOfRange::new(m.white, m.black[c]));
    let mut seen = [0u64; 4];
    let mut brightest = [0u16; 4];
    for y in 0..m.height {
        for x in 0..m.width {
            let c = m.color_at(y, x) as usize;
            let v = m.data[(y * m.width + x) as usize];
            tops[c].add(v as f32);
            seen[c] += 1;
            brightest[c] = brightest[c].max(v);
        }
    }
    let got = sidecar_raw::levels::clip_threshold(&tops);
    // How much of the frame actually reached saturation: what says whether a file can demonstrate anything
    // about highlights at all.
    let mut clipped = [0u64; 4];
    for y in 0..m.height {
        for x in 0..m.width {
            let c = m.color_at(y, x) as usize;
            let v = m.data[(y * m.width + x) as usize] as f32;
            if (v - m.black[c]) / (m.white - m.black[c]) >= got[c] {
                clipped[c] += 1;
            }
        }
    }
    let total: u64 = seen.iter().sum();
    let blown: u64 = clipped.iter().sum();
    println!(
        "  clipped: {:.4} % of samples",
        100.0 * blown as f64 / total.max(1) as f64
    );
    for c in 0..4 {
        if seen[c] == 0 {
            continue;
        }
        let span = m.white - m.black[c];
        let observed = (brightest[c] as f32 - m.black[c]) / span;
        // What `Ingest::demosaic` would have used: 98 % of the observed maximum, capped at 98.5 %.
        let old = if observed > 0.5 {
            (observed * 0.98).min(0.985)
        } else {
            0.985
        };
        println!(
            "  colour {c}: {:>11} samples \u{b7} brightest {:.4} \u{b7} histogram {:.4} \u{b7} old heuristic {:.4}",
            seen[c], observed, got[c], old
        );
    }
    Ok(())
}

fn ingest(input: &std::path::Path, out: &std::path::Path, long_edge: u32) -> Result<()> {
    use std::time::Instant;
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let develop = sidecar_engine::DevelopPass::new(&gpu)?;
    let t0 = Instant::now();
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let t_open = t0.elapsed();
    let m = raw.mosaic()?;
    let t_mosaic = t0.elapsed();
    let mul = info.color.as_shot_mul;
    let input_m = sidecar_engine::MosaicInput {
        width: m.width,
        height: m.height,
        data: &m.data,
        black: m.black,
        white: m.white,
        pattern: m.pattern,
        period: m.period,
    };
    let mut proxy = ingest
        .proxy(
            &gpu,
            &input_m,
            [mul[0] as f32, 1.0, mul[2] as f32],
            long_edge,
        )?
        .oriented(m.flip);
    let t_gpu = t0.elapsed();
    let edit: sidecar_color::EditParams = std::env::var("SIDECAR_EDIT")
        .ok()
        .map(|j| serde_json::from_str(&j))
        .transpose()?
        .unwrap_or_default();
    // With `"lens": {…}` in SIDECAR_EDIT, the file's own lens correction runs over the proxy first.
    if let Some(lens) = &edit.lens {
        let mut info = info.clone();
        sidecar_raw::read_extras(input, &mut info);
        if let Some(profile) = &info.lens_profile {
            let [sw, sh] = info.sensor_size();
            let e = sidecar_color::lens::effective(profile, lens, sw as f64 / sh as f64);
            let params = sidecar_color::lens::pack(
                &e,
                profile.center,
                [proxy.width, proxy.height],
                m.flip,
                [0.0; 2],
                [0.0; 2],
            );
            let pass = sidecar_engine::lens::LensPass::new(&gpu)?;
            let src = sidecar_engine::upload_rgba16(&gpu, &proxy);
            proxy = sidecar_engine::read_rgba16(
                &gpu,
                &pass.apply(&gpu, &src, &params, [proxy.width, proxy.height]),
            )?;
            println!(
                "lens: {} · flags {} · zoom {:.4}",
                profile.label, e.flags, e.zoom
            );
        } else {
            println!("lens: no profile in the file");
        }
    }
    let packed = sidecar_color::pack(
        &edit,
        &info.color,
        info.source_kind,
        sidecar_color::DisplaySpace::Srgb,
    );
    let img = develop.run(&gpu, &proxy, &packed.uniform, &packed.lut)?;
    write_png(&img, out)?;
    println!(
        "{} {} ({:?}) → GPU proxy {}×{} · open {:.0} ms, mosaic copy {:.0} ms, GPU ingest {:.0} ms",
        info.make,
        info.model,
        info.sensor,
        img.width,
        img.height,
        t_open.as_secs_f64() * 1e3,
        (t_mosaic - t_open).as_secs_f64() * 1e3,
        (t_gpu - t_mosaic).as_secs_f64() * 1e3
    );
    Ok(())
}
