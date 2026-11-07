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
//! `sidecar-dev pano <out.dng> <raw>… [--projection rectilinear|cylindrical|spherical] [--scale s]
//!                  [--no-crop] [--lensfun <dir>] [--png <out.png>]`  frames stitched into one linear DNG

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
        ["hilite-truth", "--full", input, evs @ ..] if !evs.is_empty() => hilite_edge(
            input.as_ref(),
            &evs.iter()
                .map(|v| v.parse())
                .collect::<Result<Vec<f32>, _>>()?,
        ),
        ["hilite-truth", input, evs @ ..] if !evs.is_empty() => hilite_truth(
            input.as_ref(),
            &evs.iter()
                .map(|v| v.parse())
                .collect::<Result<Vec<f32>, _>>()?,
        ),
        ["radial", list] => radial(list.as_ref()),
        ["levels", input] => levels(input.as_ref()),
        ["denoise-truth", clean, noisy] => denoise_truth(clean.as_ref(), noisy.as_ref()),
        ["noise", files @ ..] if !files.is_empty() => noise(files),
        ["to-dng", input, out, rest @ ..] => to_dng(
            input.as_ref(),
            out.as_ref(),
            rest.first().map(|p| std::path::Path::new(*p)),
        ),
        ["compare-png", a, b] => compare_png(a.as_ref(), b.as_ref()),
        ["align", files @ ..] if files.len() >= 2 => align(files),
        ["denoise-truth", clean, noisy] => denoise_truth(clean.as_ref(), noisy.as_ref()),
        ["noise", files @ ..] if !files.is_empty() => noise(files),
        ["to-dng", input, out, rest @ ..] => to_dng(
            input.as_ref(),
            out.as_ref(),
            rest.first().map(|p| std::path::Path::new(*p)),
        ),
        ["compare-png", a, b] => compare_png(a.as_ref(), b.as_ref()),
        ["align", files @ ..] if files.len() >= 2 => align(files),
        ["hdr", out, files @ ..] if files.len() >= 2 => hdr(out.as_ref(), files),
        ["pano", out, args @ ..] if args.len() >= 2 => pano(out.as_ref(), args),
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

/// `hilite-truth --full <raw> <ev>…`: `hilite-truth` at full resolution, where a demosaic fringe would live.
/// The frame is raised and clipped on purpose as there, both are demosaiced at full size, and the colour error
/// against the truth is reported by distance from the nearest clipped pixel: the pixels right beside a
/// clipped area are the ones a demosaic mixing clipped and measured samples would tint.
fn hilite_edge(input: &std::path::Path, evs: &[f32]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ing = sidecar_engine::Ingest::new(&gpu)?;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    let mul = info.color.as_shot_mul;
    let wb = sidecar_color::camera::resolve(&info.color, info.color.as_shot_xy());
    let run = |data: &[u16]| -> Result<sidecar_engine::RgbaF32> {
        let input_m = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        let tex = ing.demosaic(&gpu, &input_m, [mul[0] as f32, 1.0, mul[2] as f32], true)?;
        sidecar_engine::read_rgba16(&gpu, &tex)
    };
    let truth = run(&m.data)?;
    let (w, h) = (truth.width as usize, truth.height as usize);
    println!("{} {} · full resolution {w}×{h}", info.make, info.model);
    let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
    for &ev in evs {
        let k = 2f32.powf(ev);
        let raised: Vec<u16> = m
            .data
            .iter()
            .map(|&v| (blk + (v as f32 - blk).max(0.0) * k).min(m.white) as u16)
            .collect();
        let got = run(&raised)?;
        // Distance (Chebyshev, capped at 4) from the nearest pixel the raised frame clipped.
        let clipped: Vec<bool> = got.data.chunks(4).map(|p| p[3] > 0.0).collect();
        let mut dist = vec![4u8; w * h];
        for y in 0..h {
            for x in 0..w {
                if !clipped[y * w + x] {
                    continue;
                }
                for dy in -3i64..=3 {
                    for dx in -3i64..=3 {
                        let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                        if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                            continue;
                        }
                        let d = dx.unsigned_abs().max(dy.unsigned_abs()) as u8;
                        let cell = &mut dist[yy as usize * w + xx as usize];
                        *cell = (*cell).min(d);
                    }
                }
            }
        }
        let names = ["clipped", "1 px from it", "2–3 px", "farther"];
        let mut acc = [(0f64, 0f64, 0f64, 0u64); 4];
        for ((t, g), &d) in truth.data.chunks(4).zip(got.data.chunks(4)).zip(&dist) {
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
            if ts <= 1e-4 || !gs.is_finite() || gs <= 1e-9 {
                continue;
            }
            let class = match d {
                0 => 0,
                1 => 1,
                2 | 3 => 2,
                _ => 3,
            };
            let d = [gc[0] / gs - tc[0] / ts, gc[2] / gs - tc[2] / ts];
            let a = &mut acc[class];
            a.0 += (d[0] * d[0] + d[1] * d[1]).sqrt();
            a.1 += d[0];
            a.2 += d[1];
            a.3 += 1;
        }
        println!("  +{ev} EV     pixels    colour error    bias r     bias b");
        for (c, a) in acc.iter().enumerate() {
            if a.3 == 0 {
                continue;
            }
            let n = a.3 as f64;
            println!(
                "  {:<12} {:>9}    {:>10.4}    {:>+8.4}   {:>+8.4}",
                names[c],
                a.3,
                a.0 / n,
                a.1 / n,
                a.2 / n
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
/// `noise <raw>…`: each file's measured noise model (`sidecar_engine::noise`) on its full-resolution
/// demosaic, with the ratio read/gain that decides how far the fixed transform's assumption is from it
/// (the fixed transform is the model with read/gain = 0.0005 at gain 1), and the level bins it was fitted to.
fn noise(files: &[&str]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let detail = sidecar_engine::detail::Detail::new(&gpu)?;
    for f in files {
        let path: &std::path::Path = f.as_ref();
        let raw = match sidecar_raw::RawFile::open(path) {
            Ok(r) => r,
            Err(e) => {
                println!("{f}: {e}");
                continue;
            }
        };
        let info = raw.info();
        let Ok(m) = raw.mosaic() else {
            println!("{f}: not a mosaic");
            continue;
        };
        let mul = info.color.as_shot_mul;
        let input = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        let tex = ingest.demosaic(&gpu, &input, [mul[0] as f32, 1.0, mul[2] as f32], true)?;
        let side = 1024u32.min(tex.width()).min(tex.height());
        let crop = detail.crop(
            &gpu,
            &tex,
            (tex.width() - side) / 2,
            (tex.height() - side) / 2,
            side,
            side,
        );
        let lum: Vec<f32> = sidecar_engine::read_rgba16(&gpu, &crop)?
            .data
            .chunks(4)
            .map(|p| (p[0] + p[1] + p[2]) / 3.0)
            .collect();
        match sidecar_engine::noise::estimate(&lum, side as usize, side as usize) {
            Some((model, bins)) => {
                println!(
                    "{f}: {} {} ISO {} · σ² = {:.3e}·y + {:.3e} · read/gain {:.2e} · {} bins",
                    info.make,
                    info.model,
                    info.iso,
                    model.gain,
                    model.read,
                    model.read / model.gain.max(1e-12),
                    bins.len()
                );
                for b in bins {
                    println!(
                        "    level {:.5}  σ {:.5}  (model {:.5})  n {}",
                        b.level,
                        b.sigma,
                        model.sigma(b.level),
                        b.count
                    );
                }
            }
            None => println!(
                "{f}: {} {} · too few flat levels to fit",
                info.make, info.model
            ),
        }
    }
    Ok(())
}

/// `denoise-truth <clean raw> <noisy raw>`: how well each stabilization denoises, against ground truth. The
/// noisy frame's noise model is measured (`sidecar_engine::noise`), that noise is added synthetically to a
/// central 1024² crop of the clean frame — Poisson-Gaussian, per channel, so its luminance has exactly the
/// measured model — and the result is denoised under the fixed transform and under the model, across the
/// strength range. Printed per brightness band: the luminance error against the clean crop, relative to the
/// band's level. The two frames should come from the same camera, so the noise is that sensor's own.
fn denoise_truth(clean: &std::path::Path, noisy: &std::path::Path) -> Result<()> {
    use sidecar_engine::noise::{NoiseModel, Synth};
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let detail = sidecar_engine::detail::Detail::new(&gpu)?;
    let full = |path: &std::path::Path| -> Result<(_, String)> {
        let raw = sidecar_raw::RawFile::open(path)?;
        let info = raw.info();
        let m = raw.mosaic()?;
        let mul = info.color.as_shot_mul;
        let input = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        let tex = ingest.demosaic(&gpu, &input, [mul[0] as f32, 1.0, mul[2] as f32], true)?;
        Ok((tex, format!("{} {}", info.make, info.model)))
    };
    let (noisy_tex, noisy_cam) = full(noisy)?;
    let measured = detail.noise(&gpu, &noisy_tex)?;
    let Some((model, _)) = measured.model else {
        bail!("{}: no noise model could be measured", noisy.display());
    };
    let (clean_tex, clean_cam) = full(clean)?;
    let side = 1024u32.min(clean_tex.width()).min(clean_tex.height());
    let crop = detail.crop(
        &gpu,
        &clean_tex,
        (clean_tex.width() - side) / 2,
        (clean_tex.height() - side) / 2,
        side,
        side,
    );
    let truth = sidecar_engine::read_rgba16(&gpu, &crop)?;
    println!(
        "noise from {noisy_cam} ({}): σ² = {:.3e}·y + {:.3e} on luminance · onto {side}² of {clean_cam} ({})",
        noisy.display(),
        model.gain,
        model.read,
        clean.display()
    );
    // Per channel, three times the luminance's variance: the mean of three independent channels then has the
    // measured model exactly.
    let channel = NoiseModel {
        gain: 3.0 * model.gain,
        read: 3.0 * model.read,
    };
    let mut synth = Synth::new(1);
    let mut noisy_img = truth.clone();
    for (i, v) in noisy_img.data.iter_mut().enumerate() {
        if i % 4 != 3 {
            *v = synth.observe(&channel, *v);
        }
    }
    let tex = sidecar_engine::upload_rgba16(&gpu, &noisy_img);
    let noise = detail.noise(&gpu, &tex)?;
    const EDGES: [f32; 7] = [
        0.0,
        1.0 / 256.0,
        1.0 / 64.0,
        1.0 / 16.0,
        0.25,
        1.0,
        f32::MAX,
    ];
    let lum = |p: &[f32]| (p[0] + p[1] + p[2]) / 3.0;
    let errors = |img: &sidecar_engine::RgbaF32| -> Vec<f64> {
        let mut acc = vec![(0f64, 0f64, 0usize); EDGES.len() - 1];
        for (t, g) in truth.data.chunks(4).zip(img.data.chunks(4)) {
            if t[3] > 0.0 {
                continue;
            }
            let l = lum(t);
            let b = EDGES
                .windows(2)
                .position(|e| l >= e[0] && l < e[1])
                .unwrap_or(0);
            let d = (lum(g) - l) as f64;
            acc[b].0 += d * d;
            acc[b].1 += l as f64;
            acc[b].2 += 1;
        }
        acc.iter()
            .map(|&(se, sum, n)| {
                if n < 200 {
                    f64::NAN
                } else {
                    100.0 * (se / n as f64).sqrt() / (sum / n as f64)
                }
            })
            .collect()
    };
    let header: Vec<String> = EDGES
        .windows(2)
        .map(|e| {
            if e[1] == f32::MAX {
                format!(">{:.3}", e[0])
            } else {
                format!("<{:.4}", e[1])
            }
        })
        .collect();
    println!("luminance error, % of the band's level, by clean luminance band:");
    println!(
        "{:<18}{}",
        "",
        header.iter().map(|h| format!("{h:>9}")).collect::<String>()
    );
    let row = |name: &str, e: &[f64]| {
        println!(
            "{name:<18}{}",
            e.iter()
                .map(|v| if v.is_nan() {
                    format!("{:>9}", "-")
                } else {
                    format!("{v:>9.1}")
                })
                .collect::<String>()
        )
    };
    row("noisy", &errors(&noisy_img));
    let mut best = [(f64::MAX, 0.0), (f64::MAX, 0.0)];
    for strength in [20.0f32, 40.0, 60.0, 80.0, 100.0] {
        for (k, model_on) in [false, true].into_iter().enumerate() {
            let stab = noise.stabilization(model_on);
            let out = sidecar_engine::read_rgba16(
                &gpu,
                &detail.denoise(&gpu, &tex, &stab, strength, strength),
            )?;
            let e = errors(&out);
            let mean: f64 = e.iter().filter(|v| v.is_finite()).sum::<f64>()
                / e.iter().filter(|v| v.is_finite()).count().max(1) as f64;
            if mean < best[k].0 {
                best[k] = (mean, strength as f64);
            }
            row(
                &format!("{} {strength:.0}", if model_on { "model" } else { "fixed" }),
                &e,
            );
        }
    }
    println!(
        "best mean over bands: fixed {:.1} % at {:.0}, model {:.1} % at {:.0}",
        best[0].0, best[0].1, best[1].0, best[1].1
    );
    Ok(())
}

/// `to-dng <raw> <out.dng>`: the raw's full-resolution demosaic written as a linear float DNG
/// (`sidecar_raw::dng_write`), with its colour model and orientation — what a merge writes, from one frame, so
/// the writer can be checked end to end: `develop` the raw and `linear-dng` the result, and the two pictures
/// should be the same.
/// With a third argument, also writes there the demosaic developed exactly as `linear-dng` develops the file
/// (averaged down by the same whole factor to a 1600 px long edge, default edit, sRGB): the two PNGs then
/// differ only by what the file did to the pixels.
fn to_dng(
    input: &std::path::Path,
    out: &std::path::Path,
    png: Option<&std::path::Path>,
) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    let mul = info.color.as_shot_mul;
    let tex = ingest.demosaic(
        &gpu,
        &sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        },
        [mul[0] as f32, 1.0, mul[2] as f32],
        true,
    )?;
    let img = sidecar_engine::read_rgba16(&gpu, &tex)?;
    let rgb: Vec<f32> = img
        .data
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let t0 = std::time::Instant::now();
    sidecar_raw::dng_write::write(
        out,
        img.width,
        img.height,
        &rgb,
        &sidecar_raw::dng_write::DngMeta {
            make: &info.make,
            model: &info.model,
            software: "Sidecar (sidecar-dev to-dng)",
            color: &info.color,
            orientation: sidecar_raw::decoded::orientation_from_flip(m.flip),
            noise: None,
            linear_response_limit: Some(1.0),
        },
    )?;
    if let Some(png) = png {
        let (w, h) = (img.width as usize, img.height as usize);
        let f = w.max(h).div_ceil(1600);
        let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
        let mut sum = vec![0f32; ow * oh * 4];
        let mut n = vec![0f32; ow * oh];
        for y in 0..h {
            for x in 0..w {
                let d = (y / f) * ow + x / f;
                for k in 0..3 {
                    sum[d * 4 + k] += img.data[(y * w + x) * 4 + k].max(0.0);
                }
                n[d] += 1.0;
            }
        }
        for (p, k) in sum.chunks_mut(4).zip(&n) {
            p.iter_mut().for_each(|v| *v /= k);
        }
        let small = sidecar_engine::RgbaF32 {
            width: ow as u32,
            height: oh as u32,
            data: sum,
        }
        .oriented(sidecar_raw::decoded::flip_from_orientation(
            sidecar_raw::decoded::orientation_from_flip(m.flip) as i64,
        ));
        let packed = sidecar_color::pack(
            &Default::default(),
            &info.color,
            sidecar_raw::SourceKind::Raw,
            sidecar_color::DisplaySpace::Srgb,
        );
        let pass = sidecar_engine::DevelopPass::new(&gpu)?;
        write_png(&pass.run(&gpu, &small, &packed.uniform, &packed.lut)?, png)?;
    }
    println!(
        "{} {} · {}×{} → {} ({:.1} MB, {:.1} s)",
        info.make,
        info.model,
        img.width,
        img.height,
        out.display(),
        std::fs::metadata(out)?.len() as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// `compare-png <a.png> <b.png>`: two 8-bit sRGB renders of the same picture at different sizes, both
/// box-averaged in linear light to the smaller one's size, compared by ΔE2000 (mean, p99, max) — the parity
/// harness's measure, for pictures that took different routes.
fn compare_png(a: &std::path::Path, b: &std::path::Path) -> Result<()> {
    use palette::{IntoColor, Lab, LinSrgb, color_difference::Ciede2000};
    let read = |p: &std::path::Path| -> Result<(u32, u32, Vec<f32>)> {
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(p)?));
        let mut r = dec.read_info()?;
        let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
        let info = r.next_frame(&mut buf)?;
        let ch = info.color_type.samples();
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let px = buf[..info.buffer_size()]
            .chunks(ch)
            .flat_map(|p| [lin(p[0]), lin(p[1]), lin(p[2])])
            .collect();
        Ok((info.width, info.height, px))
    };
    let ((aw, ah, ap), (bw, bh, bp)) = (read(a)?, read(b)?);
    let (w, h) = (aw.min(bw), ah.min(bh));
    let shrink = |sw: u32, sh: u32, p: &[f32]| -> Vec<[f32; 3]> {
        let mut sum = vec![[0f32; 4]; (w * h) as usize];
        for y in 0..sh {
            for x in 0..sw {
                let (tx, ty) = ((x * w / sw).min(w - 1), (y * h / sh).min(h - 1));
                let d = &mut sum[(ty * w + tx) as usize];
                let i = ((y * sw + x) * 3) as usize;
                d[0] += p[i];
                d[1] += p[i + 1];
                d[2] += p[i + 2];
                d[3] += 1.0;
            }
        }
        sum.iter()
            .map(|s| [s[0] / s[3], s[1] / s[3], s[2] / s[3]])
            .collect()
    };
    let (x, y) = (shrink(aw, ah, &ap), shrink(bw, bh, &bp));
    let mut de: Vec<f32> = x
        .iter()
        .zip(&y)
        .map(|(p, q)| {
            let l1: Lab = LinSrgb::new(p[0], p[1], p[2]).into_color();
            let l2: Lab = LinSrgb::new(q[0], q[1], q[2]).into_color();
            l1.difference(l2)
        })
        .collect();
    de.sort_by(f32::total_cmp);
    let mean = de.iter().sum::<f32>() / de.len() as f32;
    println!(
        "{}×{} vs {}×{} at {w}×{h}: ΔE2000 mean {mean:.3}, p99 {:.3}, max {:.3}",
        aw,
        ah,
        bw,
        bh,
        de[de.len() * 99 / 100],
        de[de.len() - 1]
    );
    Ok(())
}

/// `align <raw> <raw>…`: each consecutive pair aligned (`sidecar_merge::align`) on 1600 px proxies of log
/// luminance — so brackets align as well as panning frames — with the inliers, their error, how much of the
/// first frame lands in the second, and the shift, turn and scale the homography amounts to.
fn align(files: &[&str]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let grey = |f: &str| -> Result<sidecar_merge::align::Grey> {
        let raw = sidecar_raw::RawFile::open(f.as_ref())?;
        let info = raw.info();
        let m = raw.mosaic()?;
        let mul = info.color.as_shot_mul;
        let p = ingest
            .proxy(
                &gpu,
                &sidecar_engine::MosaicInput {
                    width: m.width,
                    height: m.height,
                    data: &m.data,
                    black: m.black,
                    white: m.white,
                    pattern: m.pattern,
                    period: m.period,
                },
                [mul[0] as f32, 1.0, mul[2] as f32],
                1600,
            )?
            .oriented(m.flip);
        Ok(sidecar_merge::align::Grey::log_luminance(
            p.width as usize,
            p.height as usize,
            p.data.chunks(4).map(|c| (c[0] + c[1] + c[2]) / 3.0),
        ))
    };
    let mut prev = grey(files[0])?;
    for pair in files.windows(2) {
        let next = grey(pair[1])?;
        let t0 = std::time::Instant::now();
        let name = |f: &str| {
            std::path::Path::new(f)
                .file_name()
                .map_or(f.to_string(), |n| n.to_string_lossy().into_owned())
        };
        match sidecar_merge::align::align(&prev, &next) {
            Some(a) => {
                let h = a.h;
                let o = sidecar_merge::align::overlap(&h, prev.w, prev.h, next.w, next.h);
                let c = sidecar_merge::align::apply(&h, [prev.w as f64 / 2.0, prev.h as f64 / 2.0]);
                let rot = h[1][0].atan2(h[0][0]).to_degrees();
                let scale = (h[0][0] * h[1][1] - h[0][1] * h[1][0]).sqrt();
                println!(
                    "{} → {}: {} of {} matches agree (rms {:.2} px) · overlap {:.0} % · centre moves ({:+.0}, {:+.0}) px · turn {:+.2}° · scale {:.3} · {:.1} s",
                    name(pair[0]),
                    name(pair[1]),
                    a.inliers,
                    a.matches,
                    a.rms,
                    100.0 * o,
                    c[0] - prev.w as f64 / 2.0,
                    c[1] - prev.h as f64 / 2.0,
                    rot,
                    scale,
                    t0.elapsed().as_secs_f64()
                );
            }
            None => println!("{} → {}: no alignment", name(pair[0]), name(pair[1])),
        }
        prev = next;
    }
    Ok(())
}

/// `align <raw> <raw>…`: each consecutive pair aligned (`sidecar_merge::align`) on 1600 px proxies of log
/// luminance — so brackets align as well as panning frames — with the inliers, their error, how much of the
/// first frame lands in the second, and the shift, turn and scale the homography amounts to.
fn align(files: &[&str]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let grey = |f: &str| -> Result<sidecar_merge::align::Grey> {
        let raw = sidecar_raw::RawFile::open(f.as_ref())?;
        let info = raw.info();
        let m = raw.mosaic()?;
        let mul = info.color.as_shot_mul;
        let p = ingest
            .proxy(
                &gpu,
                &sidecar_engine::MosaicInput {
                    width: m.width,
                    height: m.height,
                    data: &m.data,
                    black: m.black,
                    white: m.white,
                    pattern: m.pattern,
                    period: m.period,
                },
                [mul[0] as f32, 1.0, mul[2] as f32],
                1600,
            )?
            .oriented(m.flip);
        Ok(sidecar_merge::align::Grey::log_luminance(
            p.width as usize,
            p.height as usize,
            p.data.chunks(4).map(|c| (c[0] + c[1] + c[2]) / 3.0),
        ))
    };
    let mut prev = grey(files[0])?;
    for pair in files.windows(2) {
        let next = grey(pair[1])?;
        let t0 = std::time::Instant::now();
        let name = |f: &str| {
            std::path::Path::new(f)
                .file_name()
                .map_or(f.to_string(), |n| n.to_string_lossy().into_owned())
        };
        match sidecar_merge::align::align(&prev, &next) {
            Some(a) => {
                let h = a.h;
                let o = sidecar_merge::align::overlap(&h, prev.w, prev.h, next.w, next.h);
                let c = sidecar_merge::align::apply(&h, [prev.w as f64 / 2.0, prev.h as f64 / 2.0]);
                let rot = h[1][0].atan2(h[0][0]).to_degrees();
                let scale = (h[0][0] * h[1][1] - h[0][1] * h[1][0]).sqrt();
                println!(
                    "{} → {}: {} of {} matches agree (rms {:.2} px) · overlap {:.0} % · centre moves ({:+.0}, {:+.0}) px · turn {:+.2}° · scale {:.3} · {:.1} s",
                    name(pair[0]),
                    name(pair[1]),
                    a.inliers,
                    a.matches,
                    a.rms,
                    100.0 * o,
                    c[0] - prev.w as f64 / 2.0,
                    c[1] - prev.h as f64 / 2.0,
                    rot,
                    scale,
                    t0.elapsed().as_secs_f64()
                );
            }
            None => println!("{} → {}: no alignment", name(pair[0]), name(pair[1])),
        }
        prev = next;
    }
    Ok(())
}

/// Where each CFA colour actually saturates, by the histogram of the top of its range, beside the single
/// brightest sample the old heuristics keyed off (D6/D7 in docs/research/pipeline-review-2026-10.md). On a
/// frame with blown highlights the two should agree; where they don't, the histogram is the one that
/// survives a stuck pixel.
/// `noise <raw>…`: each file's measured noise model (`sidecar_engine::noise`) on its full-resolution
/// demosaic, with the ratio read/gain that decides how far the fixed transform's assumption is from it
/// (the fixed transform is the model with read/gain = 0.0005 at gain 1), and the level bins it was fitted to.
fn noise(files: &[&str]) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let detail = sidecar_engine::detail::Detail::new(&gpu)?;
    for f in files {
        let path: &std::path::Path = f.as_ref();
        let raw = match sidecar_raw::RawFile::open(path) {
            Ok(r) => r,
            Err(e) => {
                println!("{f}: {e}");
                continue;
            }
        };
        let info = raw.info();
        let Ok(m) = raw.mosaic() else {
            println!("{f}: not a mosaic");
            continue;
        };
        let mul = info.color.as_shot_mul;
        let input = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        let tex = ingest.demosaic(&gpu, &input, [mul[0] as f32, 1.0, mul[2] as f32], true)?;
        let side = 1024u32.min(tex.width()).min(tex.height());
        let crop = detail.crop(
            &gpu,
            &tex,
            (tex.width() - side) / 2,
            (tex.height() - side) / 2,
            side,
            side,
        );
        let lum: Vec<f32> = sidecar_engine::read_rgba16(&gpu, &crop)?
            .data
            .chunks(4)
            .map(|p| (p[0] + p[1] + p[2]) / 3.0)
            .collect();
        match sidecar_engine::noise::estimate(&lum, side as usize, side as usize) {
            Some((model, bins)) => {
                println!(
                    "{f}: {} {} ISO {} · σ² = {:.3e}·y + {:.3e} · read/gain {:.2e} · {} bins",
                    info.make,
                    info.model,
                    info.iso,
                    model.gain,
                    model.read,
                    model.read / model.gain.max(1e-12),
                    bins.len()
                );
                for b in bins {
                    println!(
                        "    level {:.5}  σ {:.5}  (model {:.5})  n {}",
                        b.level,
                        b.sigma,
                        model.sigma(b.level),
                        b.count
                    );
                }
            }
            None => println!(
                "{f}: {} {} · too few flat levels to fit",
                info.make, info.model
            ),
        }
    }
    Ok(())
}

/// `denoise-truth <clean raw> <noisy raw>`: how well each stabilization denoises, against ground truth. The
/// noisy frame's noise model is measured (`sidecar_engine::noise`), that noise is added synthetically to a
/// central 1024² crop of the clean frame — Poisson-Gaussian, per channel, so its luminance has exactly the
/// measured model — and the result is denoised under the fixed transform and under the model, across the
/// strength range. Printed per brightness band: the luminance error against the clean crop, relative to the
/// band's level. The two frames should come from the same camera, so the noise is that sensor's own.
fn denoise_truth(clean: &std::path::Path, noisy: &std::path::Path) -> Result<()> {
    use sidecar_engine::noise::{NoiseModel, Synth};
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let detail = sidecar_engine::detail::Detail::new(&gpu)?;
    let full = |path: &std::path::Path| -> Result<(_, String)> {
        let raw = sidecar_raw::RawFile::open(path)?;
        let info = raw.info();
        let m = raw.mosaic()?;
        let mul = info.color.as_shot_mul;
        let input = sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        };
        let tex = ingest.demosaic(&gpu, &input, [mul[0] as f32, 1.0, mul[2] as f32], true)?;
        Ok((tex, format!("{} {}", info.make, info.model)))
    };
    let (noisy_tex, noisy_cam) = full(noisy)?;
    let measured = detail.noise(&gpu, &noisy_tex)?;
    let Some((model, _)) = measured.model else {
        bail!("{}: no noise model could be measured", noisy.display());
    };
    let (clean_tex, clean_cam) = full(clean)?;
    let side = 1024u32.min(clean_tex.width()).min(clean_tex.height());
    let crop = detail.crop(
        &gpu,
        &clean_tex,
        (clean_tex.width() - side) / 2,
        (clean_tex.height() - side) / 2,
        side,
        side,
    );
    let truth = sidecar_engine::read_rgba16(&gpu, &crop)?;
    println!(
        "noise from {noisy_cam} ({}): σ² = {:.3e}·y + {:.3e} on luminance · onto {side}² of {clean_cam} ({})",
        noisy.display(),
        model.gain,
        model.read,
        clean.display()
    );
    // Per channel, three times the luminance's variance: the mean of three independent channels then has the
    // measured model exactly.
    let channel = NoiseModel {
        gain: 3.0 * model.gain,
        read: 3.0 * model.read,
    };
    let mut synth = Synth::new(1);
    let mut noisy_img = truth.clone();
    for (i, v) in noisy_img.data.iter_mut().enumerate() {
        if i % 4 != 3 {
            *v = synth.observe(&channel, *v);
        }
    }
    let tex = sidecar_engine::upload_rgba16(&gpu, &noisy_img);
    let noise = detail.noise(&gpu, &tex)?;
    const EDGES: [f32; 7] = [
        0.0,
        1.0 / 256.0,
        1.0 / 64.0,
        1.0 / 16.0,
        0.25,
        1.0,
        f32::MAX,
    ];
    let lum = |p: &[f32]| (p[0] + p[1] + p[2]) / 3.0;
    let errors = |img: &sidecar_engine::RgbaF32| -> Vec<f64> {
        let mut acc = vec![(0f64, 0f64, 0usize); EDGES.len() - 1];
        for (t, g) in truth.data.chunks(4).zip(img.data.chunks(4)) {
            if t[3] > 0.0 {
                continue;
            }
            let l = lum(t);
            let b = EDGES
                .windows(2)
                .position(|e| l >= e[0] && l < e[1])
                .unwrap_or(0);
            let d = (lum(g) - l) as f64;
            acc[b].0 += d * d;
            acc[b].1 += l as f64;
            acc[b].2 += 1;
        }
        acc.iter()
            .map(|&(se, sum, n)| {
                if n < 200 {
                    f64::NAN
                } else {
                    100.0 * (se / n as f64).sqrt() / (sum / n as f64)
                }
            })
            .collect()
    };
    let header: Vec<String> = EDGES
        .windows(2)
        .map(|e| {
            if e[1] == f32::MAX {
                format!(">{:.3}", e[0])
            } else {
                format!("<{:.4}", e[1])
            }
        })
        .collect();
    println!("luminance error, % of the band's level, by clean luminance band:");
    println!(
        "{:<18}{}",
        "",
        header.iter().map(|h| format!("{h:>9}")).collect::<String>()
    );
    let row = |name: &str, e: &[f64]| {
        println!(
            "{name:<18}{}",
            e.iter()
                .map(|v| if v.is_nan() {
                    format!("{:>9}", "-")
                } else {
                    format!("{v:>9.1}")
                })
                .collect::<String>()
        )
    };
    row("noisy", &errors(&noisy_img));
    let mut best = [(f64::MAX, 0.0), (f64::MAX, 0.0)];
    for strength in [20.0f32, 40.0, 60.0, 80.0, 100.0] {
        for (k, model_on) in [false, true].into_iter().enumerate() {
            let stab = noise.stabilization(model_on);
            let out = sidecar_engine::read_rgba16(
                &gpu,
                &detail.denoise(&gpu, &tex, &stab, strength, strength),
            )?;
            let e = errors(&out);
            let mean: f64 = e.iter().filter(|v| v.is_finite()).sum::<f64>()
                / e.iter().filter(|v| v.is_finite()).count().max(1) as f64;
            if mean < best[k].0 {
                best[k] = (mean, strength as f64);
            }
            row(
                &format!("{} {strength:.0}", if model_on { "model" } else { "fixed" }),
                &e,
            );
        }
    }
    println!(
        "best mean over bands: fixed {:.1} % at {:.0}, model {:.1} % at {:.0}",
        best[0].0, best[0].1, best[1].0, best[1].1
    );
    Ok(())
}

/// Where each CFA colour actually saturates, by the histogram of the top of its range, beside the single
/// brightest sample the old heuristics keyed off (D6/D7 in docs/research/pipeline-review-2026-10.md). On a
/// frame with blown highlights the two should agree; where they don't, the histogram is the one that
/// survives a stuck pixel.
/// `to-dng <raw> <out.dng>`: the raw's full-resolution demosaic written as a linear float DNG
/// (`sidecar_raw::dng_write`), with its colour model and orientation — what a merge writes, from one frame, so
/// the writer can be checked end to end: `develop` the raw and `linear-dng` the result, and the two pictures
/// should be the same.
/// With a third argument, also writes there the demosaic developed exactly as `linear-dng` develops the file
/// (averaged down by the same whole factor to a 1600 px long edge, default edit, sRGB): the two PNGs then
/// differ only by what the file did to the pixels.
fn to_dng(
    input: &std::path::Path,
    out: &std::path::Path,
    png: Option<&std::path::Path>,
) -> Result<()> {
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let raw = sidecar_raw::RawFile::open(input)?;
    let info = raw.info();
    let m = raw.mosaic()?;
    let mul = info.color.as_shot_mul;
    let tex = ingest.demosaic(
        &gpu,
        &sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        },
        [mul[0] as f32, 1.0, mul[2] as f32],
        true,
    )?;
    let img = sidecar_engine::read_rgba16(&gpu, &tex)?;
    let rgb: Vec<f32> = img
        .data
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let t0 = std::time::Instant::now();
    sidecar_raw::dng_write::write(
        out,
        img.width,
        img.height,
        &rgb,
        &sidecar_raw::dng_write::DngMeta {
            make: &info.make,
            model: &info.model,
            software: "Sidecar (sidecar-dev to-dng)",
            color: &info.color,
            orientation: sidecar_raw::decoded::orientation_from_flip(m.flip),
            noise: None,
            linear_response_limit: Some(1.0),
        },
    )?;
    if let Some(png) = png {
        let (w, h) = (img.width as usize, img.height as usize);
        let f = w.max(h).div_ceil(1600);
        let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
        let mut sum = vec![0f32; ow * oh * 4];
        let mut n = vec![0f32; ow * oh];
        for y in 0..h {
            for x in 0..w {
                let d = (y / f) * ow + x / f;
                for k in 0..3 {
                    sum[d * 4 + k] += img.data[(y * w + x) * 4 + k].max(0.0);
                }
                n[d] += 1.0;
            }
        }
        for (p, k) in sum.chunks_mut(4).zip(&n) {
            p.iter_mut().for_each(|v| *v /= k);
        }
        let small = sidecar_engine::RgbaF32 {
            width: ow as u32,
            height: oh as u32,
            data: sum,
        }
        .oriented(sidecar_raw::decoded::flip_from_orientation(
            sidecar_raw::decoded::orientation_from_flip(m.flip) as i64,
        ));
        let packed = sidecar_color::pack(
            &Default::default(),
            &info.color,
            sidecar_raw::SourceKind::Raw,
            sidecar_color::DisplaySpace::Srgb,
        );
        let pass = sidecar_engine::DevelopPass::new(&gpu)?;
        write_png(&pass.run(&gpu, &small, &packed.uniform, &packed.lut)?, png)?;
    }
    println!(
        "{} {} · {}×{} → {} ({:.1} MB, {:.1} s)",
        info.make,
        info.model,
        img.width,
        img.height,
        out.display(),
        std::fs::metadata(out)?.len() as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// `compare-png <a.png> <b.png>`: two 8-bit sRGB renders of the same picture at different sizes, both
/// box-averaged in linear light to the smaller one's size, compared by ΔE2000 (mean, p99, max) — the parity
/// harness's measure, for pictures that took different routes.
fn compare_png(a: &std::path::Path, b: &std::path::Path) -> Result<()> {
    use palette::{IntoColor, Lab, LinSrgb, color_difference::Ciede2000};
    let read = |p: &std::path::Path| -> Result<(u32, u32, Vec<f32>)> {
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(p)?));
        let mut r = dec.read_info()?;
        let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
        let info = r.next_frame(&mut buf)?;
        let ch = info.color_type.samples();
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let px = buf[..info.buffer_size()]
            .chunks(ch)
            .flat_map(|p| [lin(p[0]), lin(p[1]), lin(p[2])])
            .collect();
        Ok((info.width, info.height, px))
    };
    let ((aw, ah, ap), (bw, bh, bp)) = (read(a)?, read(b)?);
    let (w, h) = (aw.min(bw), ah.min(bh));
    let shrink = |sw: u32, sh: u32, p: &[f32]| -> Vec<[f32; 3]> {
        let mut sum = vec![[0f32; 4]; (w * h) as usize];
        for y in 0..sh {
            for x in 0..sw {
                let (tx, ty) = ((x * w / sw).min(w - 1), (y * h / sh).min(h - 1));
                let d = &mut sum[(ty * w + tx) as usize];
                let i = ((y * sw + x) * 3) as usize;
                d[0] += p[i];
                d[1] += p[i + 1];
                d[2] += p[i + 2];
                d[3] += 1.0;
            }
        }
        sum.iter()
            .map(|s| [s[0] / s[3], s[1] / s[3], s[2] / s[3]])
            .collect()
    };
    let (x, y) = (shrink(aw, ah, &ap), shrink(bw, bh, &bp));
    let mut de: Vec<f32> = x
        .iter()
        .zip(&y)
        .map(|(p, q)| {
            let l1: Lab = LinSrgb::new(p[0], p[1], p[2]).into_color();
            let l2: Lab = LinSrgb::new(q[0], q[1], q[2]).into_color();
            l1.difference(l2)
        })
        .collect();
    de.sort_by(f32::total_cmp);
    let mean = de.iter().sum::<f32>() / de.len() as f32;
    println!(
        "{}×{} vs {}×{} at {w}×{h}: ΔE2000 mean {mean:.3}, p99 {:.3}, max {:.3}",
        aw,
        ah,
        bw,
        bh,
        de[de.len() * 99 / 100],
        de[de.len() - 1]
    );
    Ok(())
}

/// `hdr <out.dng> <raw>…`: a bracket merged into one linear DNG (`sidecar_merge::hdr`). The reference is the
/// median exposure; every frame is aligned to it on proxies, its exposure step measured against it, its noise
/// measured, and it is merged in two streaming passes — the second dropping samples that disagree beyond their
/// noise — then written by `sidecar_raw::dng_write`.
fn hdr(out: &std::path::Path, files: &[&str]) -> Result<()> {
    use sidecar_merge::align::{Grey, H};
    use sidecar_merge::hdr::{Accum, Frame};
    let t0 = std::time::Instant::now();
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let detail = sidecar_engine::detail::Detail::new(&gpu)?;
    let raws: Vec<sidecar_raw::RawFile> = files
        .iter()
        .map(|f| sidecar_raw::RawFile::open(f.as_ref()))
        .collect::<Result<_, _>>()?;
    // What each frame let in, by its EXIF: shutter × ISO / f².
    let light: Vec<f64> = raws
        .iter()
        .map(|r| {
            let i = r.info();
            i.shutter_s as f64 * i.iso as f64 / (i.aperture as f64).max(0.7).powi(2)
        })
        .collect();
    let mut order: Vec<usize> = (0..raws.len()).collect();
    order.sort_by(|&a, &b| light[a].total_cmp(&light[b]));
    let reference = order[order.len() / 2];
    let input = |r: &sidecar_raw::RawFile| -> Result<(sidecar_raw::Mosaic, [f32; 3])> {
        let m = r.mosaic()?;
        let mul = r.info().color.as_shot_mul;
        Ok((m, [mul[0] as f32, 1.0, mul[2] as f32]))
    };
    fn mosaic(m: &sidecar_raw::Mosaic) -> sidecar_engine::MosaicInput<'_> {
        sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        }
    }
    // Alignment on proxies of log luminance, in the sensor's own orientation.
    let proxy = |r: &sidecar_raw::RawFile| -> Result<Grey> {
        let (m, wb) = input(r)?;
        let p = ingest.proxy(&gpu, &mosaic(&m), wb, 1600)?;
        Ok(Grey::log_luminance(
            p.width as usize,
            p.height as usize,
            p.data.chunks(4).map(|c| (c[0] + c[1] + c[2]) / 3.0),
        ))
    };
    let ref_proxy = proxy(&raws[reference])?;
    // Full-resolution frames, as the merge reads them.
    /// A frame at full resolution: width, height, RGB, clip flags, and its measured noise.
    type FullFrame = (
        usize,
        usize,
        Vec<f32>,
        Vec<bool>,
        sidecar_engine::detail::Noise,
    );
    let full = |r: &sidecar_raw::RawFile| -> Result<FullFrame> {
        let (m, wb) = input(r)?;
        let tex = ingest.demosaic(&gpu, &mosaic(&m), wb, true)?;
        let noise = detail.noise(&gpu, &tex)?;
        let img = sidecar_engine::read_rgba16(&gpu, &tex)?;
        let rgb = img
            .data
            .chunks(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let clipped = img.data.chunks(4).map(|p| p[3] > 0.0).collect();
        Ok((img.width as usize, img.height as usize, rgb, clipped, noise))
    };
    let (rw, rh, ref_rgb, ref_clipped, ref_noise) = full(&raws[reference])?;
    let id: H = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let per_channel = |n: &sidecar_engine::detail::Noise| {
        n.model.map_or((3e-4, 3e-6), |(m, _)| {
            (3.0 * m.gain as f64, 3.0 * m.read as f64)
        })
    };
    // Per frame: the homography from the reference at full resolution, the scale into the reference's
    // radiance, and the noise. Scales are measured as a chain, outward from the reference in both directions,
    // each frame against its neighbour a step nearer — far apart, too few pixels are well exposed in both.
    println!(
        "reference {} ({} frames) · {rw}×{rh}",
        files[reference],
        files.len()
    );
    let n = raws.len();
    let mut plan: Vec<(H, f64, (f64, f64))> = vec![(id, 1.0, (0.0, 0.0)); n];
    plan[reference] = (id, 1.0, per_channel(&ref_noise));
    let rp = order.iter().position(|&i| i == reference).unwrap();
    let chains: [Vec<usize>; 2] = [
        order[rp + 1..].to_vec(),
        order[..rp].iter().rev().copied().collect(),
    ];
    for chain in chains {
        let (mut nb_i, mut nb_rgb, mut nb_clipped, mut nb_h, mut nb_scale) =
            (reference, ref_rgb.clone(), ref_clipped.clone(), id, 1.0);
        let (mut nb_w, mut nb_hh) = (rw, rh);
        for i in chain {
            let p = proxy(&raws[i])?;
            let s = rw as f64 / ref_proxy.w as f64;
            let h = match sidecar_merge::align::align(&ref_proxy, &p) {
                Some(a) => {
                    let hp = a.h;
                    // Proxy → full: conjugate by the scale (pixel centres at +0.5).
                    let sc: H = [
                        [s, 0.0, 0.5 * s - 0.5],
                        [0.0, s, 0.5 * s - 0.5],
                        [0.0, 0.0, 1.0],
                    ];
                    let inv: H = [
                        [1.0 / s, 0.0, (0.5 - 0.5 * s) / s],
                        [0.0, 1.0 / s, (0.5 - 0.5 * s) / s],
                        [0.0, 0.0, 1.0],
                    ];
                    let mul = |a: &H, b: &H| -> H {
                        std::array::from_fn(|i| {
                            std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())
                        })
                    };
                    println!(
                        "  {}: aligned, {} inliers, rms {:.2} px",
                        files[i], a.inliers, a.rms
                    );
                    mul(&sc, &mul(&hp, &inv))
                }
                None => {
                    println!("  {}: no alignment; taken as it is", files[i]);
                    id
                }
            };
            let (w, hh, rgb, clipped, noise) = full(&raws[i])?;
            let exif = light[nb_i] / light[i];
            let frame = |w, h, rgb, clipped, from_ref| Frame {
                w,
                h,
                rgb,
                clipped,
                scale: 1.0,
                gain: 0.0,
                read: 0.0,
                from_ref,
            };
            let measured = sidecar_merge::hdr::measured_ratio(
                &frame(nb_w, nb_hh, &nb_rgb, &nb_clipped, nb_h),
                &frame(w, hh, &rgb, &clipped, h),
                rw,
                rh,
            );
            // Half a stop from EXIF is not a bracket's rounding but a failed measurement: EXIF it is.
            let step = measured
                .filter(|m| (m.log2() - exif.log2()).abs() < 0.5)
                .unwrap_or(exif);
            let scale = nb_scale * step;
            println!(
                "    {:+.2} EV from the frame before by EXIF, {} measured → {:+.2} EV from the reference",
                -exif.log2(),
                measured.map_or("not".into(), |m| format!("{:+.2} EV", -m.log2())),
                -scale.log2()
            );
            plan[i] = (h, scale, per_channel(&noise));
            (nb_i, nb_rgb, nb_clipped, nb_h, nb_scale, nb_w, nb_hh) =
                (i, rgb, clipped, h, scale, w, hh);
        }
    }
    let mut first = None;
    for pass in 0..2 {
        let mut acc = Accum::new(rw, rh);
        for (i, r) in raws.iter().enumerate() {
            let (w, hh, rgb, clipped, _) = if i == reference {
                (rw, rh, ref_rgb.clone(), ref_clipped.clone(), ref_noise)
            } else {
                full(r)?
            };
            let (h, scale, (gain, read)) = plan[i];
            acc.add(
                &Frame {
                    w,
                    h: hh,
                    rgb: &rgb,
                    clipped: &clipped,
                    scale,
                    gain,
                    read,
                    from_ref: h,
                },
                if pass == 1 { first.as_deref() } else { None },
            );
        }
        let (est, fell) = acc.finish();
        if pass == 1 {
            let info = raws[reference].info();
            let m = raws[reference].mosaic()?;
            let darkest = plan.iter().map(|p| p.1).fold(0.0, f64::max);
            sidecar_raw::dng_write::write(
                out,
                rw as u32,
                rh as u32,
                &est,
                &sidecar_raw::dng_write::DngMeta {
                    make: &info.make,
                    model: &info.model,
                    software: "Sidecar (sidecar-dev hdr)",
                    color: &info.color,
                    orientation: sidecar_raw::decoded::orientation_from_flip(m.flip),
                    noise: None,
                    // Where no frame measured the light, the darkest frame's saturation is all there is.
                    linear_response_limit: (fell > 0).then_some(darkest * 0.95),
                },
            )?;
            let peak = est.iter().cloned().fold(0f32, f32::max);
            println!(
                "merged: peak {:.1}× the reference's white ({:+.1} EV), {} px measured by no frame · {} ({:.0} MB) in {:.0} s",
                peak,
                (peak as f64).log2(),
                fell,
                out.display(),
                std::fs::metadata(out)?.len() as f64 / 1e6,
                t0.elapsed().as_secs_f64()
            );
        }
        first = Some(est);
    }
    Ok(())
}

/// `pano <out.dng> <raw>… [options]`: frames turned about one point, stitched into one linear DNG
/// (`sidecar_merge::pano`). The lens profile is the first file's own, or lensfun's from `--lensfun <dir>`;
/// without either the solve fits distortion and vignetting itself. Frames are solved on proxies, then each
/// is demosaiced at full resolution, warped and blended in turn, so only one full frame is held at a time.
fn pano(out: &std::path::Path, args: &[&str]) -> Result<()> {
    use sidecar_merge::pano::{self, Canvas, Input, Optics, Projection};
    let t0 = std::time::Instant::now();
    let mut files = vec![];
    let (mut projection, mut scale, mut crop, mut lensfun, mut png) = (None, 1.0, true, None, None);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut value = || {
            it.next()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("{a} needs a value"))
        };
        match a {
            "--projection" => {
                let v = value()?;
                projection =
                    Some(Projection::parse(v).ok_or_else(|| anyhow::anyhow!("no projection {v}"))?)
            }
            "--scale" => scale = value()?.parse()?,
            "--no-crop" => crop = false,
            "--lensfun" => lensfun = Some(std::path::PathBuf::from(value()?)),
            "--png" => png = Some(std::path::PathBuf::from(value()?)),
            f => files.push(f),
        }
    }
    anyhow::ensure!(files.len() >= 2, "a panorama needs two frames");
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let ingest = sidecar_engine::Ingest::new(&gpu)?;
    let raws: Vec<sidecar_raw::RawFile> = files
        .iter()
        .map(|f| sidecar_raw::RawFile::open(f.as_ref()))
        .collect::<Result<_, _>>()?;
    let mut info = raws[0].info().clone();
    sidecar_raw::read_extras(files[0].as_ref(), &mut info);
    let focal = info.focal_length_mm;
    for (r, f) in raws.iter().zip(&files) {
        let fl = r.info().focal_length_mm;
        anyhow::ensure!(
            (fl - focal).abs() <= 0.01 * focal.max(1.0),
            "{f} is at {fl} mm and {} at {focal} mm: one panorama is one focal length",
            files[0]
        );
    }
    let profile = info.lens_profile.clone().or_else(|| {
        let db = sidecar_raw::lensfun::Database::load(lensfun.as_deref()?);
        db.profile(&info, info.lens_identity.as_ref()?)
    });
    let sample = |c: &Option<sidecar_color::lens::Curve>| -> Vec<f64> {
        c.as_ref().map_or(vec![], |c| {
            (0..=96)
                .map(|k| c.eval(k as f64 / 96.0 * pano::R_MAX))
                .collect()
        })
    };
    let optics = match &profile {
        Some(p) => Optics {
            center: p.center,
            distortion: sample(&p.distortion),
            ca: [sample(&p.ca_red), sample(&p.ca_blue)],
            vignetting: sample(&p.vignetting),
        },
        None => Optics {
            center: [0.5, 0.5],
            ..Default::default()
        },
    };
    println!(
        "{} {} · {} · {focal} mm · lens: {}",
        info.make,
        info.model,
        info.lens,
        profile
            .as_ref()
            .map_or("no profile; fitted".into(), |p| p.label.clone())
    );
    // One white balance for every frame (the middle one's), so the proxies compare alike.
    let mul = raws[raws.len() / 2].info().color.as_shot_mul;
    let wb = [mul[0] as f32, 1.0, mul[2] as f32];
    fn mosaic(m: &sidecar_raw::Mosaic) -> sidecar_engine::MosaicInput<'_> {
        sidecar_engine::MosaicInput {
            width: m.width,
            height: m.height,
            data: &m.data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period,
        }
    }
    let mut inputs = vec![];
    for r in &raws {
        let m = r.mosaic()?;
        let p = ingest.proxy(&gpu, &mosaic(&m), wb, 1600)?;
        let i = r.info();
        let flip = i.orientation_flip & 7;
        let up = if flip & 4 != 0 {
            [if flip & 1 != 0 { 1.0 } else { -1.0 }, 0.0]
        } else {
            [0.0, if flip & 2 != 0 { 1.0 } else { -1.0 }]
        };
        let [sw, sh] = i.sensor_size();
        inputs.push(Input {
            w: sw as usize,
            h: sh as usize,
            proxy: sidecar_merge::align::Grey::log_luminance(
                p.width as usize,
                p.height as usize,
                p.data.chunks(4).map(|c| (c[0] + c[1] + c[2]) / 3.0),
            ),
            up,
            light: i.shutter_s as f64 * i.iso as f64 / (i.aperture as f64).max(0.7).powi(2),
        });
    }
    let sol = pano::solve(&inputs, &optics, None).map_err(anyhow::Error::msg)?;
    let diag_mm = |crop: f64| 43.27 / crop;
    println!(
        "solved in {:.1} s: reference {} · focal {:.4} half-diagonals (homographies said {:.4}){} · k1 {:+.4} · rms {:.3} px of the proxy",
        t0.elapsed().as_secs_f64(),
        files[sol.reference],
        sol.lens.focal,
        sol.focal_initial,
        info.lens_identity
            .as_ref()
            .and_then(|l| l.crop_factor)
            .map_or(String::new(), |c| format!(
                " = {:.1} mm on this sensor (EXIF {focal} mm)",
                sol.lens.focal * diag_mm(c) / 2.0
            )),
        sol.lens.k1,
        sol.rms
    );
    for l in &sol.links {
        println!(
            "  {} ↔ {}: {} inliers, {} refined",
            files[l.i],
            files[l.j],
            l.inliers,
            l.a.len()
        );
    }
    for (i, f) in files.iter().enumerate() {
        match sol.rotations[i] {
            Some(_) => println!("  {f}: gain {:+.2} EV", sol.gains[i].log2()),
            None => println!("  {f}: joins nothing; left out"),
        }
    }
    if optics.vignetting.is_empty() {
        println!(
            "  vignetting fitted: {:+.2} EV at the corners",
            sol.lens.flatten(1.0).log2()
        );
    }
    let sizes: Vec<(usize, usize)> = inputs.iter().map(|i| (i.w, i.h)).collect();
    let lay =
        pano::layout(&sol, &sizes, projection, scale, 600_000_000).map_err(anyhow::Error::msg)?;
    println!(
        "{:?}, {} × {}{}, {} bands",
        lay.projection,
        lay.w,
        lay.h,
        if lay.wrap { ", a full turn" } else { "" },
        lay.levels
    );
    let mut canvas = Canvas::new(&sol, &sizes, lay.clone());
    for (i, r) in raws.iter().enumerate() {
        if sol.rotations[i].is_none() {
            continue;
        }
        let m = r.mosaic()?;
        let tex = ingest.demosaic(&gpu, &mosaic(&m), wb, true)?;
        let img = sidecar_engine::read_rgba16(&gpu, &tex)?;
        anyhow::ensure!(
            (img.width as usize, img.height as usize) == sizes[i],
            "{}: demosaiced to {}×{}, expected {:?}",
            files[i],
            img.width,
            img.height,
            sizes[i]
        );
        let rgb: Vec<f32> = img
            .data
            .chunks(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        drop(img);
        canvas.add(i, &rgb);
        println!(
            "  blended {} ({:.0} s)",
            files[i],
            t0.elapsed().as_secs_f64()
        );
    }
    let (img, seen) = canvas.finish();
    let (x0, y0, x1, y1) = if crop {
        pano::largest_rectangle(&seen, lay.w, lay.h, 8)
    } else {
        (0, 0, lay.w, lay.h)
    };
    let (w, h) = (x1 - x0, y1 - y0);
    let rgb: Vec<f32> = (y0..y1)
        .flat_map(|y| {
            img[(y * lay.w + x0) * 3..(y * lay.w + x1) * 3]
                .iter()
                .copied()
        })
        .collect();
    drop(img);
    let ri = raws[sol.reference].info();
    let limit = sol
        .rotations
        .iter()
        .zip(&sol.gains)
        .filter(|(r, _)| r.is_some())
        .map(|(_, g)| *g)
        .fold(f64::MAX, f64::min);
    sidecar_raw::dng_write::write(
        out,
        w as u32,
        h as u32,
        &rgb,
        &sidecar_raw::dng_write::DngMeta {
            make: &ri.make,
            model: &ri.model,
            software: "Sidecar (sidecar-dev pano)",
            color: &raws[raws.len() / 2].info().color,
            orientation: 1,
            noise: None,
            // A frame's white, in the reference's radiance, is its gain (more where vignetting was lifted).
            linear_response_limit: Some(limit),
        },
    )?;
    println!(
        "{} × {}{} → {} ({:.0} MB) in {:.0} s",
        w,
        h,
        if crop { " cropped" } else { "" },
        out.display(),
        std::fs::metadata(out)?.len() as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    if let Some(png) = png {
        let f = w.max(h).div_ceil(2400);
        let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
        let mut sum = vec![0f32; ow * oh * 4];
        let mut n = vec![0f32; ow * oh];
        for y in 0..h {
            for x in 0..w {
                let d = (y / f) * ow + x / f;
                for k in 0..3 {
                    sum[d * 4 + k] += rgb[(y * w + x) * 3 + k].max(0.0);
                }
                n[d] += 1.0;
            }
        }
        for (p, k) in sum.chunks_mut(4).zip(&n) {
            p.iter_mut().for_each(|v| *v /= k);
        }
        let small = sidecar_engine::RgbaF32 {
            width: ow as u32,
            height: oh as u32,
            data: sum,
        };
        let packed = sidecar_color::pack(
            &Default::default(),
            &raws[raws.len() / 2].info().color,
            sidecar_raw::SourceKind::Raw,
            sidecar_color::DisplaySpace::Srgb,
        );
        let pass = sidecar_engine::DevelopPass::new(&gpu)?;
        write_png(&pass.run(&gpu, &small, &packed.uniform, &packed.lut)?, &png)?;
    }
    Ok(())
}

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
    // What `Ingest::demosaic` actually uses: *one* number for every colour, from the brightest sample
    // anywhere in the mosaic. The per-colour "old heuristic" below is the same formula asked per channel,
    // which is what the other two decode paths do — it is not what the GPU path feeds the shaders.
    {
        let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
        let max = m.data.iter().copied().max().unwrap_or(0) as f32;
        let white_obs = ((max - blk) / (m.white - blk)).min(1.0);
        let scalar = if white_obs > 0.5 {
            (white_obs * 0.98).min(0.985)
        } else {
            0.985
        };
        println!("  one threshold for all four colours, as the GPU path uses it: {scalar:.4}");
    }
    for c in 0..4 {
        if seen[c] == 0 {
            continue;
        }
        let span = m.white - m.black[c];
        let observed = (brightest[c] as f32 - m.black[c]) / span;
        // The same formula asked per channel, as `LinearImage::rgba` and the linear-DNG path ask it:
        // 98 % of this colour's observed maximum, capped at 98.5 %.
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
