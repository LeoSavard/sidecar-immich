// SPDX-License-Identifier: Apache-2.0
//! Parity harness: the same composed WGSL, the same packed uniforms, the same source pixels, rendered by
//! wgpu natively and by WebGPU in a browser. Output must match perceptually: mean ΔE2000 ≤ 0.5, p99 ≤ 2.

use anyhow::{Context, Result, bail};
use palette::{IntoColor, Lab, LinSrgb, color_difference::Ciede2000};
use serde::{Deserialize, Serialize};
use sidecar_color::camera::{Calibration, CameraColor};
use sidecar_color::params::{EditParams, MixerBand, WhiteBalanceMode};
use sidecar_color::{DisplaySpace, SourceKind, pack};
use sidecar_engine::{DevelopPass, Gpu, RgbaF32};
use std::path::Path;

pub const MEAN_LIMIT: f32 = 0.5;
pub const P99_LIMIT: f32 = 2.0;

#[derive(Serialize, Deserialize)]
struct Case {
    name: String,
    width: u32,
    height: u32,
    /// `<name>.lens.bin` holds lens-pass uniforms, run over the source before develop.
    #[serde(default)]
    lens: bool,
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

    let mut lens = EditParams { lens: Some(sidecar_color::params::Lens { profile: "sony".into(), ..Default::default() }), ..graded.clone() };
    lens.lens.as_mut().unwrap().lateral_ca = 150.0;

    vec![
        ("as-shot-srgb".into(), nikon_d850(), SourceKind::Raw, EditParams::default(), DisplaySpace::Srgb, None),
        ("graded-p3".into(), nikon_d850(), SourceKind::Raw, graded, DisplaySpace::DisplayP3, None),
        ("xtrans-dark".into(), fujifilm_xt4(), SourceKind::Raw, dark, DisplaySpace::Srgb, None),
        ("heic-default".into(), heic(), SourceKind::Developed, EditParams::default(), DisplaySpace::DisplayP3, None),
        ("heic-graded".into(), heic(), SourceKind::Developed, phone, DisplaySpace::Srgb, None),
        ("lens-zv1".into(), nikon_d850(), SourceKind::Raw, lens, DisplaySpace::Srgb, Some(zv1())),
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
        println!("{name}: white balance {:.0} K, tint {:+.1}{}", packed.temperature_k, packed.tint, if lens { ", lens pass" } else { "" });
        index.push(Case { name, width: src.width, height: src.height, lens });
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
        let img = sidecar_engine::read_rgba16(&gpu, &pass.render(&gpu, &tex, &uniform, &lut)?)?;
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
