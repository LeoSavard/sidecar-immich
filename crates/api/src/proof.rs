// SPDX-License-Identifier: Apache-2.0
//! Soft proofing (DESIGN.md §15, "Soft proofing"): how the picture will look printed through an output profile, and
//! which of its colours the print can't hold.
//!
//! The editor shows the photo in sRGB or Display P3. A proof is that display's colour through the printer profile
//! and back — display → print, by the chosen intent, with black point compensation if asked; print → display,
//! relative colorimetric, or absolute to simulate the paper's white and the ink's black — composed by LittleCMS into
//! one table over the display's encoded colour (`proof_table`). The gamut warning comes from the same chain: the
//! same transform with LittleCMS's gamut check, run with two different alarm colours, so a node that comes back as
//! the alarm both times is out of gamut. The browser applies the table with the LUT pass (`client/look.wgsl`), its
//! alpha marking the out-of-gamut colours, so the warning cannot disagree with the proof beside it.
//!
//! Profiles are uploaded like LUTs and kept in `<state>/proofs/` under the first 16 hex digits of their SHA-256.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use lcms2::{
    CIExyY, CIExyYTRIPLE, Flags, InfoType, Intent, Locale, PixelFormat, Profile,
    ProfileClassSignature, ThreadContext, ToneCurve, Transform,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sidecar_color::DisplaySpace;

use crate::App;
use crate::session::ApiUser;

/// The proof table's size on a side: 33³ nodes, as LittleCMS's own precalculated transforms use.
pub const PROOF_SIZE: u32 = 33;
/// The largest profile accepted; real printer profiles are well under 10 MB.
pub const PROFILE_LIMIT: usize = 32 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProofInfo {
    pub id: String,
    /// The file name it was uploaded under, without its extension.
    pub name: String,
    /// The profile's own description.
    pub description: Option<String>,
}

/// The intent from the display to the print. Absolute colorimetric is not offered here: it belongs to the
/// way back, as `paper`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProofIntent {
    #[default]
    Perceptual,
    Relative,
    Saturation,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofOptions {
    #[serde(default)]
    pub display: DisplaySpace,
    #[serde(default)]
    pub intent: ProofIntent,
    /// Black point compensation from the display's black to the print's.
    #[serde(default)]
    pub bpc: bool,
    /// Simulate paper and ink: the way back absolute colorimetric, so the paper's white and the ink's black show
    /// as themselves rather than as the display's white and black.
    #[serde(default)]
    pub paper: bool,
}

fn display_profile(ctx: &ThreadContext, display: DisplaySpace) -> Result<Profile<ThreadContext>> {
    Ok(match display {
        DisplaySpace::Srgb => Profile::new_srgb_context(ctx),
        DisplaySpace::DisplayP3 => {
            let xy = |x, y| CIExyY { x, y, Y: 1.0 };
            // The sRGB curve as ICC parametric type 4: Y = (aX + b)^g above d, cX below.
            let curve = ToneCurve::new_parametric(
                4,
                &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
            )
            .context("the sRGB curve")?;
            Profile::new_rgb_context(
                ctx,
                &xy(0.3127, 0.3290),
                &CIExyYTRIPLE {
                    Red: xy(0.680, 0.320),
                    Green: xy(0.265, 0.690),
                    Blue: xy(0.150, 0.060),
                },
                &[&curve, &curve, &curve],
            )
            .context("the Display P3 profile")?
        }
    })
}

fn intent(i: ProofIntent) -> Intent {
    match i {
        ProofIntent::Perceptual => Intent::Perceptual,
        ProofIntent::Relative => Intent::RelativeColorimetric,
        ProofIntent::Saturation => Intent::Saturation,
    }
}

/// The nodes of the table: the display's encoded colour, red fastest.
fn grid() -> Vec<[f32; 3]> {
    let n = PROOF_SIZE;
    let m = (n - 1) as f32;
    (0..n * n * n)
        .map(|i| [i % n, (i / n) % n, i / (n * n)].map(|v| v as f32 / m))
        .collect()
}

/// Whether LittleCMS can proof through `icc`: it opens, and is a profile one prints or displays through.
pub fn check_profile(icc: &[u8]) -> Result<Option<String>> {
    let ctx = ThreadContext::new();
    let p = Profile::new_icc_context(&ctx, icc)
        .map_err(|e| anyhow::anyhow!("not an ICC profile LittleCMS can read: {e}"))?;
    match p.device_class() {
        ProfileClassSignature::OutputClass
        | ProfileClassSignature::DisplayClass
        | ProfileClassSignature::ColorSpaceClass => {}
        c => bail!(
            "a {c:?} profile describes no output to proof against; a printer's or a display's does"
        ),
    }
    let disp = display_profile(&ctx, DisplaySpace::Srgb)?;
    Transform::<[f32; 3], [f32; 3], ThreadContext>::new_proofing_context(
        &ctx,
        &disp,
        PixelFormat::RGB_FLT,
        &disp,
        PixelFormat::RGB_FLT,
        &p,
        Intent::RelativeColorimetric,
        Intent::RelativeColorimetric,
        Flags::SOFT_PROOFING,
    )
    .map_err(|e| anyhow::anyhow!("LittleCMS can't proof through it: {e}"))?;
    Ok(p.info(InfoType::Description, Locale::none()))
}

/// The proof as a table over the display's encoded colour, `PROOF_SIZE` on a side, RGBA, red fastest: RGB the
/// proofed colour, alpha 1 where the print can't hold the colour and 0 where it can.
pub fn proof_table(icc: &[u8], o: ProofOptions) -> Result<Vec<[f32; 4]>> {
    let nodes = grid();
    let back = if o.paper {
        Intent::AbsoluteColorimetric
    } else {
        Intent::RelativeColorimetric
    };
    let flags = if o.bpc {
        Flags::SOFT_PROOFING | Flags::BLACKPOINT_COMPENSATION
    } else {
        Flags::SOFT_PROOFING
    };
    let ctx = ThreadContext::new();
    let printer = Profile::new_icc_context(&ctx, icc).map_err(|e| anyhow::anyhow!("{e}"))?;
    let disp = display_profile(&ctx, o.display)?;
    let proof: Transform<[f32; 3], [f32; 3], ThreadContext> = Transform::new_proofing_context(
        &ctx,
        &disp,
        PixelFormat::RGB_FLT,
        &disp,
        PixelFormat::RGB_FLT,
        &printer,
        intent(o.intent),
        back,
        flags,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut proofed = vec![[0f32; 3]; nodes.len()];
    proof.transform_pixels(&nodes, &mut proofed);

    // The gamut check, twice over, with alarms no proof gives both of. Out of gamut is colorimetric — whether the
    // print can match the colour at all, before any intent or black point compensation decides what to do with
    // it — so a display black deeper than the ink's is out, as it is in LittleCMS's check whatever the flags.
    let nodes16: Vec<[u16; 3]> = nodes
        .iter()
        .map(|c| c.map(|v| (v * 65535.0).round() as u16))
        .collect();
    let mut alarmed = Vec::new();
    for alarm in [[1u16, 2, 3], [7, 11, 13]] {
        let mut ctx = ThreadContext::new();
        let mut codes = [0u16; 16];
        codes[..3].copy_from_slice(&alarm);
        ctx.set_alarm_codes(codes);
        let printer = Profile::new_icc_context(&ctx, icc).map_err(|e| anyhow::anyhow!("{e}"))?;
        let disp = display_profile(&ctx, o.display)?;
        let check: Transform<[u16; 3], [u16; 3], ThreadContext> = Transform::new_proofing_context(
            &ctx,
            &disp,
            PixelFormat::RGB_16,
            &disp,
            PixelFormat::RGB_16,
            &printer,
            Intent::RelativeColorimetric,
            Intent::RelativeColorimetric,
            Flags::SOFT_PROOFING | Flags::GAMUT_CHECK,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut out = vec![[0u16; 3]; nodes.len()];
        check.transform_pixels(&nodes16, &mut out);
        alarmed.push(out.into_iter().map(|c| c == alarm).collect::<Vec<_>>());
    }
    Ok(proofed
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let out = alarmed[0][i] && alarmed[1][i];
            [c[0], c[1], c[2], if out { 1.0 } else { 0.0 }]
        })
        .collect())
}

fn dir(app: &App) -> PathBuf {
    app.config.server.state_dir.join("proofs")
}

fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `GET /api/proofs`: every stored profile, by name.
pub async fn list(State(app): State<Arc<App>>, ApiUser(_user): ApiUser) -> Response {
    let mut out: Vec<ProofInfo> = std::fs::read_dir(dir(&app))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// `POST /api/proofs?name=…`, the ICC profile as the body.
pub async fn upload(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    let icc = body.to_vec();
    let checked = tokio::task::spawn_blocking({
        let icc = icc.clone();
        move || check_profile(&icc)
    })
    .await;
    let description = match checked {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let stem = q.name.trim();
    let stem = stem
        .strip_suffix(".icc")
        .or_else(|| stem.strip_suffix(".icm"))
        .unwrap_or(stem);
    let name: String = stem.chars().take(120).collect();
    let info = ProofInfo {
        id: hex::encode(Sha256::digest(&icc))[..16].to_string(),
        name: if name.is_empty() {
            "Profile".into()
        } else {
            name
        },
        description,
    };
    let stored = (|| -> Result<()> {
        let d = dir(&app);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(format!("{}.icc", info.id)), &icc)?;
        std::fs::write(
            d.join(format!("{}.json", info.id)),
            serde_json::to_vec(&info)?,
        )?;
        Ok(())
    })();
    match stored {
        Ok(()) => Json(info).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Couldn't store the profile: {e:#}"),
        )
            .into_response(),
    }
}

/// `GET /api/proofs/{id}/table?display=…&intent=…&bpc=…&paper=…`: the proof, in the format of `GET /api/luts/{id}`
/// (the side as a little-endian u32, the domain's six floats, then RGBA floats, red fastest), alpha the gamut warning.
pub async fn table(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Path(id): Path<String>,
    Query(o): Query<ProofOptions>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(icc) = std::fs::read(dir(&app).join(format!("{id}.icc"))) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let table = match tokio::task::spawn_blocking(move || proof_table(&icc, o)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let mut out = Vec::with_capacity(28 + table.len() * 16);
    out.extend_from_slice(&PROOF_SIZE.to_le_bytes());
    for v in [0f32, 0.0, 0.0, 1.0, 1.0, 1.0]
        .iter()
        .chain(table.iter().flatten())
    {
        out.extend_from_slice(&v.to_le_bytes());
    }
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        out,
    )
        .into_response()
}

/// `DELETE /api/proofs/{id}`.
pub async fn remove(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let d = dir(&app);
    let _ = std::fs::remove_file(d.join(format!("{id}.json")));
    match std::fs::remove_file(d.join(format!("{id}.icc"))) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A printer stand-in LittleCMS builds itself: an RGB output profile with a small gamut (primaries well inside
    /// sRGB's) and a dull white, so the gamut warning and the paper simulation have something to show. Real
    /// printer profiles are licensed and stay out of the repository; `sidecar-dev proof` runs one from disk.
    fn small_printer() -> Vec<u8> {
        let ctx = ThreadContext::new();
        let xy = |x, y| CIExyY { x, y, Y: 1.0 };
        let curve = ToneCurve::new(2.2);
        let mut p = Profile::new_rgb_context(
            &ctx,
            &xy(0.3127, 0.3290),
            &CIExyYTRIPLE {
                Red: xy(0.55, 0.33),
                Green: xy(0.30, 0.55),
                Blue: xy(0.17, 0.12),
            },
            &[&curve, &curve, &curve],
        )
        .unwrap();
        p.set_device_class(ProfileClassSignature::OutputClass);
        p.icc().unwrap()
    }

    fn node(table: &[[f32; 4]], rgb: [u32; 3]) -> [f32; 4] {
        let n = PROOF_SIZE;
        table[(rgb[0] + n * (rgb[1] + n * rgb[2])) as usize]
    }

    #[test]
    fn proofs_through_a_small_gamut_and_warns_where_it_cannot_hold() {
        let icc = small_printer();
        assert!(check_profile(&icc).is_ok());
        let o = ProofOptions {
            display: DisplaySpace::Srgb,
            intent: ProofIntent::Relative,
            bpc: false,
            paper: false,
        };
        let t = proof_table(&icc, o).unwrap();
        let m = PROOF_SIZE - 1;
        // Greys are in any gamut, and come back as themselves.
        for g in [0, m / 4, m / 2, m] {
            let c = node(&t, [g, g, g]);
            assert_eq!(c[3], 0.0, "grey {g} flagged");
            let want = g as f32 / m as f32;
            assert!(
                (c[0] - want).abs() < 0.01 && (c[2] - want).abs() < 0.01,
                "grey {g}: {c:?}"
            );
        }
        // sRGB's pure red is outside primaries this small: flagged, and proofed duller.
        let red = node(&t, [m, 0, 0]);
        assert_eq!(red[3], 1.0, "{red:?}");
        assert!(
            red[1] > 0.05 || red[2] > 0.05,
            "red kept its purity: {red:?}"
        );
        // A muted colour inside it is not.
        let muted = node(&t, [m / 2 + 2, m / 2, m / 2 - 2]);
        assert_eq!(muted[3], 0.0, "{muted:?}");
        let share = t.iter().filter(|c| c[3] > 0.5).count() as f64 / t.len() as f64;
        assert!((0.2..0.95).contains(&share), "out of gamut share {share}");
    }

    #[test]
    fn refuses_profiles_with_no_output() {
        assert!(check_profile(b"not a profile").is_err());
        let ctx = ThreadContext::new();
        let mut p = Profile::new_srgb_context(&ctx);
        p.set_device_class(ProfileClassSignature::InputClass);
        assert!(check_profile(&p.icc().unwrap()).is_err());
    }

    #[test]
    fn a_p3_display_keeps_its_greys() {
        let t = proof_table(
            &small_printer(),
            ProofOptions {
                display: DisplaySpace::DisplayP3,
                intent: ProofIntent::Perceptual,
                bpc: true,
                paper: false,
            },
        )
        .unwrap();
        let m = PROOF_SIZE - 1;
        let c = node(&t, [m / 2, m / 2, m / 2]);
        assert!(
            (c[0] - c[1]).abs() < 0.01 && (c[1] - c[2]).abs() < 0.01,
            "{c:?}"
        );
    }

    /// A real printer profile from disk (`SIDECAR_PROOF_ICC`), which the repository can't carry: what it does to
    /// white, black and the primaries, and how much of the display's colour it can't print.
    #[test]
    #[ignore]
    fn a_real_profile() {
        let path = std::env::var("SIDECAR_PROOF_ICC").expect("SIDECAR_PROOF_ICC names a profile");
        let icc = std::fs::read(&path).unwrap();
        println!("{path}: {:?}", check_profile(&icc).unwrap());
        let m = PROOF_SIZE - 1;
        for (intent, bpc, paper) in [
            (ProofIntent::Perceptual, false, false),
            (ProofIntent::Relative, false, false),
            (ProofIntent::Relative, true, false),
            (ProofIntent::Perceptual, false, true),
        ] {
            let t = proof_table(
                &icc,
                ProofOptions {
                    display: DisplaySpace::Srgb,
                    intent,
                    bpc,
                    paper,
                },
            )
            .unwrap();
            let share = t.iter().filter(|c| c[3] > 0.5).count() as f64 / t.len() as f64;
            let f = |c: [f32; 4]| {
                format!(
                    "{:.3} {:.3} {:.3}{}",
                    c[0],
                    c[1],
                    c[2],
                    if c[3] > 0.5 { " out" } else { "" }
                )
            };
            println!(
                "{intent:?} bpc {bpc} paper {paper}: white {}, black {}, red {}, blue {}; {:.0} % out of gamut",
                f(node(&t, [m, m, m])),
                f(node(&t, [0, 0, 0])),
                f(node(&t, [m, 0, 0])),
                f(node(&t, [0, 0, m])),
                share * 100.0
            );
        }
    }
}
