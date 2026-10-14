// SPDX-License-Identifier: Apache-2.0
//! `sidecar-dev parity fixtures <dir>`    write synthetic sources + packed uniforms for each parity case
//! `sidecar-dev parity native <dir> <out>`  render every case with wgpu (Vulkan on the server)
//! `sidecar-dev parity compare <out>`     compare <case>.native.f16 with <case>.browser.f16 (ΔE2000)
//! `sidecar-dev develop <raw> <out.png> [long_edge]`  decode → proxy → GPU develop (as shot) → PNG
//! `sidecar-dev png <in.f16> <width> <height> <out.png>`  sRGB-encode a display-linear RGBA16F dump for viewing
//! `sidecar-dev lens <file>…`             the lens correction and identity each file yields
//! `sidecar-dev radial <list>`            measured radial falloff per RAW (vignetting checks)
//! `sidecar-dev camera-jpeg <list>`       a RAW's lens profile checked against the camera's own JPEG

mod parity;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["ingest", input, out, rest @ ..] => ingest(input.as_ref(), out.as_ref(), rest.first().map_or(Ok(1600), |v| v.parse())?),
        ["develop", input, out, rest @ ..] => develop(input.as_ref(), out.as_ref(), rest.first().map_or(Ok(1600), |v| v.parse())?),
        ["png", input, w, h, out] => {
            let img = sidecar_engine::RgbaF32::from_f16_bytes(w.parse()?, h.parse()?, &std::fs::read(input)?);
            write_png(&img, out.as_ref())
        }
        ["radial", list] => radial(list.as_ref()),
        ["linear-dng", input, out, rest @ ..] => {
            let input: &std::path::Path = input.as_ref();
            let edge: u32 = rest.first().map_or(Ok(1600), |v| v.parse())?;
            let t0 = std::time::Instant::now();
            let info = sidecar_raw::linear_dng::info(input, &sidecar_raw::decoded::exif(input))?;
            let img = sidecar_raw::linear_dng::decode(input, edge)?;
            let t_decode = t0.elapsed();
            let src = sidecar_engine::RgbaF32 { width: img.width, height: img.height, data: img.rgba }.oriented(info.orientation_flip);
            let gpu = sidecar_engine::Gpu::new_headless()?;
            let pass = sidecar_engine::DevelopPass::new(&gpu)?;
            let packed = sidecar_color::pack(&Default::default(), &info.color, info.source_kind, sidecar_color::DisplaySpace::Srgb);
            let out_img = pass.run(&gpu, &src, &packed.uniform, &packed.lut)?;
            write_png(&out_img, out.as_ref())?;
            println!(
                "{} {} · {}×{} → {}×{} · as shot {:.0} K {:+.0} · decode {:.1} s",
                info.make, info.model, info.width, info.height, out_img.width, out_img.height, packed.temperature_k, packed.tint, t_decode.as_secs_f64()
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
                let has = |c: Option<&sidecar_color::lens::Curve>, r: f64| c.map_or("-".to_string(), |c| format!("{:.4}", c.eval(r)));
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
                    if p.is_some_and(|p| p.required) { " · required" } else { "" },
                );
            }
            Ok(())
        }
        ["camera-jpeg", list] => camera_jpeg(list.as_ref()),
        ["parity", "fixtures", dir] => parity::fixtures(dir.as_ref()),
        ["parity", "native", dir, out] => parity::native(dir.as_ref(), out.as_ref()),
        ["parity", "compare", dir, out] => parity::compare(dir.as_ref(), out.as_ref()),
        _ => bail!("usage: sidecar-dev parity fixtures <dir> | parity native <dir> <out> | parity compare <dir> <out>"),
    }
}

fn write_png(img: &sidecar_engine::RgbaF32, out: &std::path::Path) -> Result<()> {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    let rgb: Vec<u8> = img.data.chunks(4).flat_map(|p| [enc(p[0]), enc(p[1]), enc(p[2])]).collect();
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), img.width, img.height);
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
    let linear = raw.develop_linear(true)?;
    let t_demosaic = t0.elapsed();
    let proxy = sidecar_engine::RgbaF32 { width: linear.width, height: linear.height, data: linear.rgba() }.downsampled(long_edge);
    let t_proxy = t0.elapsed();
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let pass = sidecar_engine::DevelopPass::new(&gpu)?;
    let edit: sidecar_color::EditParams = match std::env::var("SIDECAR_EDIT") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => Default::default(),
    };
    let packed = sidecar_color::pack(&edit, &info.color, info.source_kind, sidecar_color::DisplaySpace::Srgb);
    let src = proxy;
    let t_gpu0 = std::time::Instant::now();
    let img = pass.run(&gpu, &src, &packed.uniform, &packed.lut)?;
    let t_gpu = t_gpu0.elapsed();
    write_png(&img, out)?;
    println!(
        "{} {} ({:?}) {}×{} → proxy {}×{} · as shot {:.0} K {:+.0} · open {:.0} ms, half-size demosaic {:.0} ms, proxy {:.0} ms, GPU develop {:.0} ms",
        info.make, info.model, info.sensor, info.width, info.height, img.width, img.height, packed.temperature_k, packed.tint,
        t_open.as_secs_f64() * 1e3, (t_demosaic - t_open).as_secs_f64() * 1e3, (t_proxy - t_demosaic).as_secs_f64() * 1e3, t_gpu.as_secs_f64() * 1e3
    );
    Ok(())
}

/// Measured vignetting: for each RAW listed in `list` (one path per line), the mean log2 of the green channel
/// in 20 rings of radius (1 = half-diagonal), relative to the centre ring, as one JSON line. Averaged over
/// many photos, the scene term evens out and what is left is the lens falloff — the check on how a maker's
/// vignetting table is scaled, where no second opinion from Adobe exists.
fn radial(list: &std::path::Path) -> Result<()> {
    const RINGS: usize = 20;
    for path in std::fs::read_to_string(list)?.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(mut raw) = sidecar_raw::RawFile::open(path.as_ref()) else { continue };
        let Ok(img) = raw.develop_linear(true) else { continue };
        let (w, h) = (img.width as f64, img.height as f64);
        let hd = (w * w + h * h).sqrt() / 2.0;
        let (mut sum, mut n) = ([0f64; RINGS], [0u64; RINGS]);
        for (i, p) in img.rgb.chunks(3).enumerate() {
            let g = p[1];
            if !(256..60000).contains(&g) {
                continue;
            }
            let (x, y) = ((i as f64 % w) + 0.5 - w / 2.0, (i as f64 / w).floor() + 0.5 - h / 2.0);
            let ring = (((x * x + y * y).sqrt() / hd) * RINGS as f64).min(RINGS as f64 - 1.0) as usize;
            sum[ring] += (g as f64).log2();
            n[ring] += 1;
        }
        if n[0] < 100 {
            continue;
        }
        let centre = sum[0] / n[0] as f64;
        let profile: Vec<Option<f64>> = (0..RINGS).map(|r| (n[r] > 100).then(|| sum[r] / n[r] as f64 - centre)).collect();
        println!("{}", serde_json::json!({ "path": path, "profile": profile }));
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
    let lensfun = std::env::var("SIDECAR_LENSFUN").ok().map(|d| sidecar_raw::lensfun::Database::load(d.as_ref()));
    for path in std::fs::read_to_string(list)?.lines().filter(|l| !l.trim().is_empty()) {
        let out = std::process::Command::new("exiftool").arg("-j").args(sidecar_raw::lens::TAGS).arg(path).output()?;
        let Some(tags) = serde_json::from_slice::<serde_json::Value>(&out.stdout).ok().and_then(|v| v.get(0).cloned()) else { continue };
        // The largest rendering the camera embedded: Nikon's full-size JpgFromRaw, else the preview.
        let mut preview = std::process::Command::new("exiftool").args(["-b", "-JpgFromRaw"]).arg(path).output()?.stdout;
        if preview.len() < 1000 {
            preview = std::process::Command::new("exiftool").args(["-b", "-PreviewImage"]).arg(path).output()?.stdout;
        }
        let Ok(mut raw) = sidecar_raw::RawFile::open(path.as_ref()) else { continue };
        let mut info = raw.info();
        let size = if info.orientation_flip & 4 != 0 { [info.height, info.width] } else { [info.width, info.height] };
        let profile = match &lensfun {
            Some(db) => {
                sidecar_raw::read_extras(path.as_ref(), &mut info);
                db.profile(&info, info.lens_identity.as_ref().unwrap_or(&Default::default()))
            }
            None => sidecar_raw::lens::from_tags(&tags, size),
        };
        let Some(profile) = profile else { continue };
        let Some(dist) = profile.distortion.clone() else { continue };
        let Ok(img) = raw.develop_linear(true) else { continue };
        let Ok(jpeg) = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview)).decode() else { continue };
        let Some((jw, jh)) = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview)).dimensions().or_else(|| {
            let mut d = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&preview));
            d.decode_headers().ok()?;
            d.dimensions()
        }) else {
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
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
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
                    let u = [((x as f64 + 0.5) - jw as f64 / 2.0) / jhd * zoom, ((y as f64 + 0.5) - jh as f64 / 2.0) / jhd * zoom];
                    let r = (u[0] * u[0] + u[1] * u[1]).sqrt();
                    let s = curve(r);
                    let (sx, sy) = (rw as f64 / 2.0 + u[0] * s * rhd, rh as f64 / 2.0 + u[1] * s * rhd);
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
            let (ma, mb) = (s.iter().map(|p| p.1).sum::<f64>() / n, s.iter().map(|p| p.2).sum::<f64>() / n);
            let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
            for p in s {
                ab += (p.1 - ma) * (p.2 - mb);
                aa += (p.1 - ma).powi(2);
                bb += (p.2 - mb).powi(2);
            }
            ab / (aa * bb).sqrt()
        };
        let best = |curve: &dyn Fn(f64) -> f64| -> (f64, f64) {
            (0..=40).map(|i| 0.9 + i as f64 * 0.005).map(|z| (z, ncc(&samples(curve, z)))).fold((1.0, -1.0), |a, b| if b.1 > a.1 { b } else { a })
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
        let mut curve: Vec<(f64, f64)> = centre.iter_mut().enumerate().filter(|(_, v)| v.len() >= 30).filter_map(|(i, v)| Some((lo + (i as f64 + 0.5) / bins as f64 * (hi - lo), median(v)?))).collect();
        for i in 1..curve.len() {
            curve[i].1 = curve[i].1.max(curve[i - 1].1);
        }
        if curve.len() < 8 {
            continue;
        }
        // JPEG log2 → the RAW log2 the centre would have needed to render it.
        let invert = |j: f64| -> Option<f64> {
            let k = curve.windows(2).position(|w| j >= w[0].1 && j <= w[1].1 && w[1].1 > w[0].1)?;
            let (a, b) = (curve[k], curve[k + 1]);
            Some(a.0 + (j - a.1) / (b.1 - a.1) * (b.0 - a.0))
        };
        let mut rings: Vec<Vec<f64>> = vec![Vec::new(); RINGS];
        for p in &s {
            if let Some(g) = invert(p.1) {
                rings[((p.0 * RINGS as f64) as usize).min(RINGS - 1)].push(g - p.2);
            }
        }
        let gain: Vec<Option<f64>> = rings.iter_mut().map(|v| if v.len() >= 50 { median(v) } else { None }).collect();
        println!("{}", serde_json::json!({ "path": path, "profile": profile.id, "ncc": with, "nccWithout": without, "zoom": zoom, "stretch": stretch, "gainEv": gain }));
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
    let input_m = sidecar_engine::MosaicInput { width: m.width, height: m.height, data: &m.data, black: m.black, white: m.white, pattern: m.pattern, period: m.period };
    let mut proxy = ingest.proxy(&gpu, &input_m, [mul[0] as f32, 1.0, mul[2] as f32], long_edge)?.oriented(m.flip);
    let t_gpu = t0.elapsed();
    let edit: sidecar_color::EditParams = std::env::var("SIDECAR_EDIT").ok().map(|j| serde_json::from_str(&j)).transpose()?.unwrap_or_default();
    // With `"lens": {…}` in SIDECAR_EDIT, the file's own lens correction runs over the proxy first.
    if let Some(lens) = &edit.lens {
        let mut info = info.clone();
        sidecar_raw::read_extras(input, &mut info);
        if let Some(profile) = &info.lens_profile {
            let [sw, sh] = info.sensor_size();
            let e = sidecar_color::lens::effective(profile, lens, sw as f64 / sh as f64);
            let params = sidecar_color::lens::pack(&e, profile.center, [proxy.width, proxy.height], m.flip, [0.0; 2], [0.0; 2]);
            let pass = sidecar_engine::lens::LensPass::new(&gpu)?;
            let src = sidecar_engine::upload_rgba16(&gpu, &proxy);
            proxy = sidecar_engine::read_rgba16(&gpu, &pass.apply(&gpu, &src, &params, [proxy.width, proxy.height]))?;
            println!("lens: {} · flags {} · zoom {:.4}", profile.label, e.flags, e.zoom);
        } else {
            println!("lens: no profile in the file");
        }
    }
    let packed = sidecar_color::pack(&edit, &info.color, info.source_kind, sidecar_color::DisplaySpace::Srgb);
    let img = develop.run(&gpu, &proxy, &packed.uniform, &packed.lut)?;
    write_png(&img, out)?;
    println!(
        "{} {} ({:?}) → GPU proxy {}×{} · open {:.0} ms, mosaic copy {:.0} ms, GPU ingest {:.0} ms",
        info.make, info.model, info.sensor, img.width, img.height, t_open.as_secs_f64() * 1e3, (t_mosaic - t_open).as_secs_f64() * 1e3, (t_gpu - t_mosaic).as_secs_f64() * 1e3
    );
    Ok(())
}
