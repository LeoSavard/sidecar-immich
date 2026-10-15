// SPDX-License-Identifier: Apache-2.0
//! Parity harness: the same composed WGSL, the same packed uniforms, the same source pixels, rendered by
//! wgpu natively and by WebGPU in a browser. Output must match perceptually: mean ΔE2000 ≤ 0.5, p99 ≤ 2.

use anyhow::{Context, Result, bail};
use palette::{IntoColor, Lab, LinSrgb, color_difference::Ciede2000};
use serde::{Deserialize, Serialize};
use sidecar_color::camera::{Calibration, CameraColor};
use sidecar_color::params::{EditParams, MaskAdjustment, MixerBand, WhiteBalanceMode};
use sidecar_color::{DisplaySpace, SourceKind, pack};
use sidecar_engine::{DevelopPass, Gpu, RgbaF32};
use std::path::Path;

pub const MEAN_LIMIT: f32 = 0.5;
pub const P99_LIMIT: f32 = 2.0;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    width: u32,
    height: u32,
    /// `<name>.lens.bin` holds lens-pass uniforms, run over the source before develop.
    #[serde(default)]
    lens: bool,
    /// `<name>.mask<i>.u8`, one byte a pixel at `mask_size`, `<name>.masks.bin` their uniforms and
    /// `<name>.curves.f32` the layers' curves.
    #[serde(default)]
    masks: u32,
    #[serde(default)]
    mask_size: [u32; 2],
    /// `<name>.mono.f32` holds black and white's table over the colour cube.
    #[serde(default)]
    mono: bool,
}

/// Masks for the parity case, at half the image's size so both renderers interpolate them: a soft disc and a
/// diagonal ramp.
const MASK_SIZE: [u32; 2] = [256, 128];

fn synthetic_masks() -> [Vec<u8>; 2] {
    let [w, h] = MASK_SIZE;
    let at = |f: &dyn Fn(f64, f64) -> f64| (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| (f((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64).clamp(0.0, 1.0) * 255.0).round() as u8).collect::<Vec<u8>>();
    let disc = at(&|u, v| 1.0 - (((u - 0.4) * 2.0).hypot(v - 0.5) * 3.0 - 0.6).clamp(0.0, 1.0));
    let ramp = at(&|u, v| u * 0.7 + v * 0.3);
    [disc, ramp]
}

/// The ZV-1 at 9.4 mm from its own tables (sidecar_raw::lens), with a vignetting curve added so every part
/// of the lens pass — warp, per-channel CA, gain — runs in both renderers.
fn zv1() -> sidecar_color::lens::LensProfile {
    use sidecar_color::lens::Curve;
    let knots = |v: &[f64], scale: f64| Curve(v.iter().enumerate().map(|(i, &x)| [i as f64 / (v.len() - 1) as f64, 1.0 + x * scale]).collect());
    sidecar_color::lens::LensProfile {
        id: "sony".into(),
        label: "From the file (Sony)".into(),
        center: [0.5, 0.5],
        distortion: Some(knots(&[1265.0, 1218.0, 1083.0, 860.0, 562.0, 209.0, -179.0, -580.0, -972.0, -1353.0, -1725.0], 1.0 / 16384.0)),
        ca_red: Some(knots(&[0.0, 0.0, 128.0, 128.0, 128.0, 256.0, 384.0, 512.0, 896.0, 1152.0, 1536.0], 1.0 / 2097152.0)),
        ca_blue: Some(knots(&[0.0, 896.0, 1024.0, 1024.0, 768.0, 512.0, 256.0, 0.0, 128.0, 0.0, 256.0], 1.0 / 2097152.0)),
        vignetting: Some(Curve(vec![[0.0, 1.0], [0.5, 1.15], [1.0, 1.9]])),
        required: true,
    }
}

fn adobe(m: [f64; 9]) -> [[f64; 3]; 3] {
    let m = m.map(|v| v / 10000.0);
    [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]
}

fn nikon_d850() -> CameraColor {
    CameraColor {
        as_shot_mul: [2.0996, 1.0, 1.4453],
        calibrations: vec![Calibration { temperature_k: 6504.0, xyz_to_camera: adobe([10405.0, -3755.0, -1270.0, -5461.0, 13787.0, 1793.0, -1040.0, 2015.0, 6785.0]) }],
    }
}

fn fujifilm_xt4() -> CameraColor {
    CameraColor {
        as_shot_mul: [1.8672, 1.0, 1.7109],
        calibrations: vec![Calibration { temperature_k: 6504.0, xyz_to_camera: adobe([13426.0, -6334.0, -1177.0, -4244.0, 12136.0, 2371.0, -580.0, 1303.0, 5980.0]) }],
    }
}

/// Synthetic camera-native source: exposure ramp, hue sweep, brightness × saturation grid, clipped highlights.
fn synthetic(camera: &CameraColor, width: u32, height: u32) -> RgbaF32 {
    let m = camera.as_shot_mul;
    let neutral = [1.0 / m[0], 1.0, 1.0 / m[2]];
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let fx = x as f64 / (width - 1) as f64;
            let band = y * 4 / height;
            let fy = ((y * 4) % height) as f64 / (height / 4) as f64 / 4.0;
            let (rgb, clip) = match band {
                0 => {
                    let v = 2f64.powf(-12.0 + 15.0 * fx);
                    (neutral.map(|n| n * v), 0.0)
                }
                1 => {
                    let h = std::f64::consts::TAU * fx;
                    let s = [0.0, 1.0, 2.0].map(|k| 0.5 + 0.5 * (h + k * std::f64::consts::TAU / 3.0).cos());
                    ([s[0] * neutral[0], s[1], s[2] * neutral[2]].map(|v| 0.25 * v), 0.0)
                }
                2 => {
                    let v = 2f64.powf(-4.0 + 7.0 * fx);
                    let sat = fy * 4.0;
                    let c = [1.0, 0.35, 0.1].map(|p| 1.0 - sat * (1.0 - p));
                    ([c[0] * neutral[0], c[1], c[2] * neutral[2]].map(|v2| v2 * v), 0.0)
                }
                _ => {
                    let v = 0.5 + 1.5 * fx;
                    ([v.min(1.0) * 0.9, v.min(1.0), (v * 1.1).min(1.0) * 0.8], (fx * 1.4 - 0.2).clamp(0.0, 1.0))
                }
            };
            data.extend_from_slice(&[rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, clip as f32]);
        }
    }
    RgbaF32 { width, height, data }
}

type Fixture = (String, CameraColor, SourceKind, EditParams, DisplaySpace, Option<sidecar_color::lens::LensProfile>);

fn cases() -> Vec<Fixture> {
    let mut graded = EditParams::default();
    graded.white_balance.mode = WhiteBalanceMode::Custom;
    graded.white_balance.temperature_k = 4300.0;
    graded.white_balance.tint = 12.0;
    graded.exposure.ev = 1.3;
    graded.exposure.highlight_blend = 1.0;
    graded.tone.contrast = 1.4;
    graded.tone.curve = vec![[0.0, 0.02], [0.25, 0.18], [0.7, 0.8], [1.0, 0.97]];
    graded.mixer.blue = MixerBand { hue: 0.0, saturation: 0.0, luminance: -40.0 };
    graded.mixer.orange = MixerBand { hue: 0.0, saturation: 30.0, luminance: 10.0 };
    graded.mixer.green = MixerBand { hue: 50.0, saturation: -20.0, luminance: 0.0 };

    let mut dark = EditParams::default();
    dark.exposure.ev = -2.0;
    dark.tone.contrast = 0.7;
    dark.tone.black_ev = -8.0;
    dark.tone.white_ev = 4.0;

    // A developed file (an iPhone HEIC's Display P3): the display-referred tone path, with every control moved.
    let mut phone = EditParams::default();
    phone.exposure.ev = 0.6;
    phone.tone.contrast = 1.3;
    phone.tone.white_ev = 3.5;
    phone.tone.black_ev = -9.0;
    phone.tone.curve = vec![[0.0, 0.0], [0.25, 0.22], [0.75, 0.8], [1.0, 1.0]];
    phone.mixer.orange = MixerBand { hue: 0.0, saturation: 20.0, luminance: 5.0 };
    let heic = || sidecar_raw::SourceSpace::DisplayP3.camera_color();

    // Two masks, one inverted, every control moved: the mask stage in both renderers.
    let mut masked = graded.clone();
    masked.masks = vec![
        MaskAdjustment { mask: "subject".into(), exposure: 1.0, contrast: 30.0, saturation: -40.0, temperature: 40.0, ..Default::default() },
        MaskAdjustment { mask: "skin".into(), invert: true, amount: 70.0, exposure: -0.7, tint: -30.0, saturation: 25.0, ..Default::default() },
    ];

    // Layers reaching the whole edit: their own white and black points, curves and mixer, on a RAW and on a
    // developed file, whose tone path the layer's parameters blend through differently.
    let layer_mixer = sidecar_color::params::Mixer {
        blue: MixerBand { hue: -30.0, saturation: 60.0, luminance: 20.0 },
        red: MixerBand { hue: 20.0, saturation: -50.0, luminance: -10.0 },
        ..Default::default()
    };
    let layers = vec![
        MaskAdjustment { mask: "subject".into(), whites: -60.0, blacks: 40.0, curve: vec![[0.0, 0.05], [0.25, 0.32], [0.75, 0.7], [1.0, 0.95]], exposure: 0.4, ..Default::default() },
        MaskAdjustment { mask: "brush".into(), invert: true, amount: 80.0, mixer: layer_mixer, contrast: -20.0, whites: 30.0, ..Default::default() },
    ];
    let layered = EditParams { masks: layers.clone(), ..graded.clone() };
    let phone_layered = EditParams { masks: layers, ..phone.clone() };

    // Film rendering: the per-channel curves blended with the neutral path, with and without layers on top.
    let film = |e: &EditParams| EditParams { film: Some(sidecar_color::params::Film { amount: 80.0, interlayer: 60.0 }), ..e.clone() };
    let (graded_film, layered_film) = (film(&graded), film(&layered));

    // Black and white: a film type behind a filter, read from the table over the colour cube; with layers too.
    let mono = |e: &EditParams, film: &str, filter: &str| EditParams { monochrome: Some(sidecar_color::params::Monochrome { film: film.into(), filter: filter.into(), ..Default::default() }), ..e.clone() };
    let (graded_mono, layered_mono) = (mono(&graded, "panchromatic", "red"), mono(&layered, "orthochromatic", "none"));

    // Grain: the hash and both octaves, in black and white, in film colour and on a developed file.
    let grain = |e: &EditParams, amount: f64, size: f64| EditParams { grain: Some(sidecar_color::params::Grain { amount, size }), ..e.clone() };
    let (grain_mono, grain_film, grain_heic) = (grain(&graded_mono, 80.0, 100.0), grain(&graded_film, 60.0, 30.0), grain(&phone, 100.0, 100.0));

    let mut lens = EditParams { lens: Some(sidecar_color::params::Lens { profile: "sony".into(), ..Default::default() }), ..graded.clone() };
    lens.lens.as_mut().unwrap().lateral_ca = 150.0;

    vec![
        ("as-shot-srgb".into(), nikon_d850(), SourceKind::Raw, EditParams::default(), DisplaySpace::Srgb, None),
        ("graded-p3".into(), nikon_d850(), SourceKind::Raw, graded, DisplaySpace::DisplayP3, None),
        ("xtrans-dark".into(), fujifilm_xt4(), SourceKind::Raw, dark, DisplaySpace::Srgb, None),
        ("heic-default".into(), heic(), SourceKind::Developed, EditParams::default(), DisplaySpace::DisplayP3, None),
        ("heic-graded".into(), heic(), SourceKind::Developed, phone, DisplaySpace::Srgb, None),
        ("lens-zv1".into(), nikon_d850(), SourceKind::Raw, lens, DisplaySpace::Srgb, Some(zv1())),
        ("masks".into(), nikon_d850(), SourceKind::Raw, masked, DisplaySpace::Srgb, None),
        ("layers".into(), nikon_d850(), SourceKind::Raw, layered, DisplaySpace::Srgb, None),
        ("heic-layers".into(), heic(), SourceKind::Developed, phone_layered, DisplaySpace::DisplayP3, None),
        ("film".into(), nikon_d850(), SourceKind::Raw, graded_film, DisplaySpace::Srgb, None),
        ("film-layers".into(), nikon_d850(), SourceKind::Raw, layered_film, DisplaySpace::DisplayP3, None),
        ("mono-red".into(), nikon_d850(), SourceKind::Raw, graded_mono, DisplaySpace::Srgb, None),
        ("mono-ortho-layers".into(), nikon_d850(), SourceKind::Raw, layered_mono, DisplaySpace::Srgb, None),
        ("grain-mono".into(), nikon_d850(), SourceKind::Raw, grain_mono, DisplaySpace::Srgb, None),
        ("grain-film".into(), nikon_d850(), SourceKind::Raw, grain_film, DisplaySpace::Srgb, None),
        ("grain-heic".into(), heic(), SourceKind::Developed, grain_heic, DisplaySpace::DisplayP3, None),
    ]
}

pub fn fixtures(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut index = Vec::new();
    for (name, camera, source, edit, display, profile) in cases() {
        let src = synthetic(&camera, 512, 256);
        let packed = pack(&edit, &camera, source, display);
        std::fs::write(dir.join(format!("{name}.src.f16")), src.to_f16_bytes())?;
        std::fs::write(dir.join(format!("{name}.uniform.bin")), &packed.uniform)?;
        std::fs::write(dir.join(format!("{name}.lut.f32")), bytemuck::cast_slice(&packed.lut))?;
        let lens = match (&profile, &edit.lens) {
            (Some(p), Some(l)) => {
                let e = sidecar_color::lens::effective(p, l, src.width as f64 / src.height as f64);
                let u = sidecar_color::lens::pack(&e, p.center, [src.width, src.height], 0, [0.0; 2], [0.0; 2]);
                std::fs::write(dir.join(format!("{name}.lens.bin")), bytemuck::bytes_of(&u))?;
                true
            }
            _ => false,
        };
        let mono = !packed.mono_lut.is_empty();
        if mono {
            std::fs::write(dir.join(format!("{name}.mono.f32")), bytemuck::cast_slice(&packed.mono_lut))?;
        }
        let masks = edit.masks.len().min(2) as u32;
        if masks > 0 {
            for (i, m) in synthetic_masks().iter().take(masks as usize).enumerate() {
                std::fs::write(dir.join(format!("{name}.mask{i}.u8")), m)?;
            }
            std::fs::write(dir.join(format!("{name}.masks.bin")), bytemuck::bytes_of(&packed.masks))?;
            std::fs::write(dir.join(format!("{name}.curves.f32")), bytemuck::cast_slice(&packed.mask_curves))?;
        }
        println!("{name}: white balance {:.0} K, tint {:+.1}{}{}", packed.temperature_k, packed.tint, if lens { ", lens pass" } else { "" }, if masks > 0 { format!(", {masks} masks") } else { String::new() });
        index.push(Case { name, width: src.width, height: src.height, lens, masks, mask_size: if masks > 0 { MASK_SIZE } else { [0; 2] }, mono });
    }
    std::fs::write(dir.join("index.json"), serde_json::to_vec_pretty(&index)?)?;
    Ok(())
}

fn load_index(dir: &Path) -> Result<Vec<Case>> {
    Ok(serde_json::from_slice(&std::fs::read(dir.join("index.json")).context("index.json — run `parity fixtures` first")?)?)
}

pub fn native(dir: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let gpu = Gpu::new_headless()?;
    println!("native adapter: {} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend);
    let pass = DevelopPass::new(&gpu)?;
    let lens_pass = sidecar_engine::lens::LensPass::new(&gpu)?;
    for case in load_index(dir)? {
        let src = RgbaF32::from_f16_bytes(case.width, case.height, &std::fs::read(dir.join(format!("{}.src.f16", case.name)))?);
        let uniform = std::fs::read(dir.join(format!("{}.uniform.bin", case.name)))?;
        let lut: Vec<f32> = bytemuck::cast_slice(&std::fs::read(dir.join(format!("{}.lut.f32", case.name)))?).to_vec();
        let mut tex = sidecar_engine::upload_rgba16(&gpu, &src);
        if case.lens {
            let bytes = std::fs::read(dir.join(format!("{}.lens.bin", case.name)))?;
            let params: sidecar_color::lens::LensParams = bytemuck::pod_read_unaligned(&bytes);
            tex = lens_pass.apply(&gpu, &tex, &params, [case.width, case.height]);
        }
        let mut mask_textures = Vec::new();
        for i in 0..case.masks {
            let bytes = std::fs::read(dir.join(format!("{}.mask{i}.u8", case.name)))?;
            let img = RgbaF32 { width: case.mask_size[0], height: case.mask_size[1], data: bytes.iter().flat_map(|&v| [v as f32 / 255.0; 4]).collect() };
            mask_textures.push(Some(sidecar_engine::upload_rgba16(&gpu, &img)));
        }
        let curves: Vec<f32> = if case.masks > 0 { bytemuck::cast_slice(&std::fs::read(dir.join(format!("{}.curves.f32", case.name)))?).to_vec() } else { Vec::new() };
        let mono: Vec<f32> = if case.mono { bytemuck::cast_slice(&std::fs::read(dir.join(format!("{}.mono.f32", case.name)))?).to_vec() } else { Vec::new() };
        let masks = if case.masks > 0 {
            let params: sidecar_color::params::MaskParams = bytemuck::pod_read_unaligned(&std::fs::read(dir.join(format!("{}.masks.bin", case.name)))?);
            sidecar_engine::Masks { params, textures: mask_textures.iter().map(Option::as_ref).collect(), curves: &curves, mono: &mono }
        } else {
            sidecar_engine::Masks { mono: &mono, ..sidecar_engine::Masks::none() }
        };
        let img = sidecar_engine::read_rgba16(&gpu, &pass.render(&gpu, &tex, &uniform, &lut, &masks)?)?;
        std::fs::write(out.join(format!("{}.native.f16", case.name)), img.to_f16_bytes())?;
    }
    Ok(())
}

fn lab(p: &[f32]) -> Lab {
    LinSrgb::new(p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0)).into_color()
}

pub fn compare(dir: &Path, out: &Path) -> Result<()> {
    let mut failed = false;
    for case in load_index(dir)? {
        let read = |kind: &str| -> Result<RgbaF32> {
            let path = out.join(format!("{}.{kind}.f16", case.name));
            Ok(RgbaF32::from_f16_bytes(case.width, case.height, &std::fs::read(&path).with_context(|| path.display().to_string())?))
        };
        let (a, b) = (read("native")?, read("browser")?);
        let mut de: Vec<f32> = a.data.chunks(4).zip(b.data.chunks(4)).map(|(p, q)| lab(p).difference(lab(q))).collect();
        de.sort_by(f32::total_cmp);
        let mean = de.iter().sum::<f32>() / de.len() as f32;
        let p99 = de[(de.len() as f32 * 0.99) as usize];
        let max = *de.last().unwrap();
        let ok = mean <= MEAN_LIMIT && p99 <= P99_LIMIT;
        failed |= !ok;
        println!("{:<14} mean ΔE {mean:.4}  p99 {p99:.4}  max {max:.4}  {}", case.name, if ok { "pass" } else { "FAIL" });
    }
    if failed {
        bail!("parity outside limits (mean ≤ {MEAN_LIMIT}, p99 ≤ {P99_LIMIT})");
    }
    Ok(())
}
