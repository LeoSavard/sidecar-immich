// SPDX-License-Identifier: Apache-2.0
//! The models Sidecar can run, and which of them this server has (DESIGN.md §12, "Models"; ROADMAP "Models on
//! demand"). Settings → Models lists them: what each enables, its licence and what that licence restricts.
//!
//! Two ways a model's weights arrive. **Shipped**: cleared for distribution and baked into the `-ml` image. **On
//! demand**: offered for download from the publisher's own URL, pinned by SHA-256, never shipped — weights
//! whose licence allows a person to use them but not Sidecar to redistribute them. Only weights ever arrive this
//! way: a model's graph is compiled into `sidecar-ml`, so what can run is this list, fixed per release.
//!
//! The models folder (`SIDECAR_MODELS_DIR`, by default `models/` in the cache) is scanned rather than recorded:
//! a weights file copied in by hand counts the same as a download. Each file there carries a stamp naming the
//! converter it was made for; after an upgrade a stale one reads as *needs update* rather than loading into a
//! structure that changed under it. No model is offered on demand yet — the first comes with the feature that
//! needs it — so this release lists the shipped four.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The burn-onnx version whose generated graphs this build runs; a stamp naming another is out of date.
pub const CONVERTER: &str = "burn-onnx 0.22.0-pre.4";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Shipped,
    /// Fetched from `url` and checked against `sha256` before conversion.
    #[allow(dead_code)]
    OnDemand {
        url: &'static str,
        sha256: &'static str,
    },
}

pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    /// What it makes possible, in the editor's words.
    pub enables: &'static str,
    pub licence: &'static str,
    /// What the licence restricts, if anything does.
    pub restrictions: Option<&'static str>,
    /// The weights file `sidecar-ml` loads.
    pub file: &'static str,
    pub delivery: Delivery,
}

pub const MODELS: &[Model] = &[
    Model {
        id: "birefnet",
        name: "BiRefNet lite",
        enables: "The subject mask",
        licence: "MIT",
        restrictions: None,
        file: "birefnet.bpk",
        delivery: Delivery::Shipped,
    },
    Model {
        id: "multiclass",
        name: "MediaPipe multi-class selfie segmentation",
        enables: "Hair, skin and clothes masks",
        licence: "Apache-2.0",
        restrictions: None,
        file: "multiclass.bpk",
        delivery: Delivery::Shipped,
    },
    Model {
        id: "yunet",
        name: "YuNet",
        enables: "Faces, for eye-first focus peaking and face masks",
        licence: "MIT",
        restrictions: None,
        file: "yunet.bpk",
        delivery: Delivery::Shipped,
    },
    Model {
        id: "depth",
        name: "Depth Anything V2 Small",
        enables: "The depth mask, lens blur and haze by distance",
        licence: "Apache-2.0",
        restrictions: None,
        file: "depth.bpk",
        delivery: Delivery::Shipped,
    },
];

/// A file's stamp: the model and the converter it was made for, beside the weights as `<file>.json`.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Stamp {
    pub model: String,
    pub converter: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Present and current.
    Installed,
    /// Shipped, but this image has no `sidecar-ml` or no weights for it: the plain image.
    NotInThisImage,
    /// On demand and not downloaded.
    NotDownloaded,
    /// In the models folder, made for another converter or for another model.
    NeedsUpdate,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: &'static str,
    pub name: &'static str,
    pub enables: &'static str,
    pub licence: &'static str,
    pub restrictions: Option<&'static str>,
    pub delivery: &'static str,
    pub state: State,
}

/// The folder downloaded or copied-in weights live in.
pub fn user_dir(cache: &Path) -> PathBuf {
    std::env::var_os("SIDECAR_MODELS_DIR").map_or_else(|| cache.join("models"), PathBuf::from)
}

/// Each model and where this server stands with it. `image` is the `-ml` image's weights folder (or `None`
/// without `sidecar-ml`); `user` is the models folder.
pub fn status(image: Option<&Path>, user: &Path) -> Vec<Status> {
    MODELS
        .iter()
        .map(|m| {
            let in_user = user.join(m.file).is_file();
            let stamp: Option<Stamp> = std::fs::read(user.join(format!("{}.json", m.file)))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let current = stamp
                .as_ref()
                .is_some_and(|s| s.model == m.id && s.converter == CONVERTER);
            let state = match (
                m.delivery,
                image.is_some_and(|d| d.join(m.file).is_file()),
                in_user,
            ) {
                // The image's own copy of a shipped model is current by construction.
                (Delivery::Shipped, true, _) => State::Installed,
                (_, _, true) if current => State::Installed,
                (_, _, true) => State::NeedsUpdate,
                (Delivery::Shipped, ..) => State::NotInThisImage,
                (Delivery::OnDemand { .. }, ..) => State::NotDownloaded,
            };
            Status {
                id: m.id,
                name: m.name,
                enables: m.enables,
                licence: m.licence,
                restrictions: m.restrictions,
                delivery: match m.delivery {
                    Delivery::Shipped => "shipped",
                    Delivery::OnDemand { .. } => "on-demand",
                },
                state,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(list: &[Status], id: &str) -> State {
        list.iter().find(|s| s.id == id).unwrap().state
    }

    /// The image's weights count as installed; without them a shipped model is "not in this image"; a copy in the
    /// models folder counts only with a stamp for this model and this converter.
    #[test]
    fn scans_the_image_and_the_models_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (image, user) = (tmp.path().join("image"), tmp.path().join("user"));
        std::fs::create_dir_all(&image).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(image.join("birefnet.bpk"), b"w").unwrap();
        let stamp = |model: &str, converter: &str| {
            serde_json::to_vec(&Stamp {
                model: model.into(),
                converter: converter.into(),
            })
            .unwrap()
        };
        // Copied in by hand, stamped for this build.
        std::fs::write(user.join("yunet.bpk"), b"w").unwrap();
        std::fs::write(user.join("yunet.bpk.json"), stamp("yunet", CONVERTER)).unwrap();
        // Made for an older converter.
        std::fs::write(user.join("depth.bpk"), b"w").unwrap();
        std::fs::write(
            user.join("depth.bpk.json"),
            stamp("depth", "burn-onnx 0.20"),
        )
        .unwrap();
        // No stamp at all.
        std::fs::write(user.join("multiclass.bpk"), b"w").unwrap();

        let s = status(Some(&image), &user);
        assert_eq!(state(&s, "birefnet"), State::Installed);
        assert_eq!(state(&s, "yunet"), State::Installed);
        assert_eq!(state(&s, "depth"), State::NeedsUpdate);
        assert_eq!(state(&s, "multiclass"), State::NeedsUpdate);

        let plain = status(None, &tmp.path().join("nowhere"));
        assert!(plain.iter().all(|m| m.state == State::NotInThisImage));
    }

    /// Every model names a licence, and no weights without one are listed: downloading does not create a
    /// permission nobody granted.
    #[test]
    fn every_model_has_a_licence() {
        for m in MODELS {
            assert!(!m.licence.is_empty() && m.licence != "none", "{}", m.id);
            if let Delivery::OnDemand { url, sha256 } = m.delivery {
                assert!(url.starts_with("https://"), "{}", m.id);
                assert_eq!(sha256.len(), 64, "{}", m.id);
            }
        }
    }
}
