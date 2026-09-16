// SPDX-License-Identifier: Apache-2.0
//! `sidecar-dev shaders <dir>`            compose the browser's WGSL entry points into <dir>
//! `sidecar-dev parity fixtures <dir>`    write synthetic sources + packed uniforms for each parity case
//! `sidecar-dev parity native <dir> <out>`  render every case with wgpu (Vulkan on the server)
//! `sidecar-dev parity compare <out>`     compare <case>.native.f16 with <case>.browser.f16 (ΔE2000)
//! `sidecar-dev develop <raw> <out.png> [long_edge]`  decode → proxy → GPU develop (as shot) → PNG
//! `sidecar-dev png <in.f16> <width> <height> <out.png>`  sRGB-encode a display-linear RGBA16F dump for viewing

mod parity;

use anyhow::{Result, bail};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["shaders", dir] => {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir)?;
            for entry in sidecar_shaders::WEB_ENTRIES {
                let path = dir.join(sidecar_shaders::composed_name(entry));
                std::fs::write(&path, sidecar_shaders::compose(entry)?)?;
                println!("wrote {}", path.display());
            }
            Ok(())
        }
        ["ingest", input, out, rest @ ..] => ingest(input.as_ref(), out.as_ref(), rest.first().map_or(Ok(1600), |v| v.parse())?),
        ["develop", input, out, rest @ ..] => develop(input.as_ref(), out.as_ref(), rest.first().map_or(Ok(1600), |v| v.parse())?),
        ["png", input, w, h, out] => {
            let img = sidecar_engine::RgbaF32::from_f16_bytes(w.parse()?, h.parse()?, &std::fs::read(input)?);
            write_png(&img, out.as_ref())
        }
        ["parity", "fixtures", dir] => parity::fixtures(dir.as_ref()),
        ["parity", "native", dir, out] => parity::native(dir.as_ref(), out.as_ref()),
        ["parity", "compare", dir, out] => parity::compare(dir.as_ref(), out.as_ref()),
        _ => bail!("usage: sidecar-dev shaders <dir> | parity fixtures <dir> | parity native <dir> <out> | parity compare <dir> <out>"),
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
    let proxy = sidecar_raw::proxy::build(&linear, long_edge);
    let t_proxy = t0.elapsed();
    let gpu = sidecar_engine::Gpu::new_headless()?;
    let pass = sidecar_engine::DevelopPass::new(&gpu)?;
    let edit: sidecar_color::EditParams = match std::env::var("SIDECAR_EDIT") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => Default::default(),
    };
    let packed = sidecar_color::pack(&edit, &info.color, sidecar_color::DisplaySpace::Srgb);
    let src = sidecar_engine::RgbaF32 { width: proxy.width, height: proxy.height, data: proxy.rgba };
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
    let proxy = ingest.proxy(&gpu, &input_m, [mul[0] as f32, 1.0, mul[2] as f32], long_edge)?.oriented(m.flip);
    let t_gpu = t0.elapsed();
    let edit: sidecar_color::EditParams = std::env::var("SIDECAR_EDIT").ok().map(|j| serde_json::from_str(&j)).transpose()?.unwrap_or_default();
    let packed = sidecar_color::pack(&edit, &info.color, sidecar_color::DisplaySpace::Srgb);
    let img = develop.run(&gpu, &proxy, &packed.uniform, &packed.lut)?;
    write_png(&img, out)?;
    println!(
        "{} {} ({:?}) → GPU proxy {}×{} · open {:.0} ms, mosaic copy {:.0} ms, GPU ingest {:.0} ms",
        info.make, info.model, info.sensor, img.width, img.height, t_open.as_secs_f64() * 1e3, (t_mosaic - t_open).as_secs_f64() * 1e3, (t_gpu - t_mosaic).as_secs_f64() * 1e3
    );
    Ok(())
}
