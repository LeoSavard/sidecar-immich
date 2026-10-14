// SPDX-License-Identifier: Apache-2.0
//! Against a real photo: a child seen from behind, facing a mirror, her reflection's face in the upper right.
//! Needs the weights (`dev/ml-models`) and `testdata/cache/img1024.rgb`; skipped without them.

use super::*;

/// Runs `test` on a thread with the stack the models need, or skips it.
fn with_models(test: impl FnOnce(Ml, Picture) + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            if let Some((ml, photo)) = setup() {
                test(
                    ml,
                    Picture {
                        width: 1024,
                        height: 1024,
                        rgb: &photo,
                    },
                );
            }
        })
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

fn setup() -> Option<(Ml, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let models = std::env::var_os("SIDECAR_ML_MODELS")
        .map(PathBuf::from)
        .unwrap_or(root.join("target/ml-models"));
    let photo = std::fs::read(root.join("testdata/cache/img1024.rgb")).ok();
    if !models.join("yunet.bpk").is_file() || photo.is_none() {
        eprintln!(
            "skipped: no weights in {} or no test photo",
            models.display()
        );
        return None;
    }
    Some((
        Ml::new(&models, Precision::Auto).expect("GPU"),
        photo.unwrap(),
    ))
}

fn at(planes: &Planes, (x, y): (f32, f32), channel: usize) -> u8 {
    let (px, py) = (
        (x * planes.width as f32) as usize,
        (y * planes.height as f32) as usize,
    );
    planes.data[(py * planes.width as usize + px) * planes.channels as usize + channel]
}

#[test]
fn finds_the_face_in_the_mirror() {
    with_models(|mut ml, photo| {
        let faces = ml.faces(photo).unwrap();
        assert_eq!(faces.len(), 1, "{faces:?}");
        let [x, y, w, h] = faces[0].bounds;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        assert!(
            faces[0].score > 0.8 && (0.8..0.9).contains(&cx) && (0.2..0.3).contains(&cy),
            "{faces:?}"
        );
        // Eyes above the nose, the nose above the mouth.
        let l = faces[0].landmarks;
        assert!(l[0][1] < l[2][1] && l[2][1] < l[3][1], "{l:?}");
    });
}

#[test]
fn separates_the_subject_from_the_door() {
    with_models(|mut ml, photo| {
        let mask = ml.subject(photo, 512).unwrap();
        assert_eq!((mask.width, mask.height, mask.channels), (512, 512, 1));
        assert!(at(&mask, (0.35, 0.4), 0) > 240, "her back");
        assert!(at(&mask, (0.05, 0.2), 0) < 15, "the door");
    });
}

#[test]
fn tells_hair_from_clothes() {
    with_models(|mut ml, photo| {
        let parts = ml.parts(photo, 256).unwrap();
        assert_eq!(parts.channels as usize, PARTS.len());
        let best = |p| {
            (0..PARTS.len())
                .max_by_key(|&c| at(&parts, p, c))
                .map(|c| PARTS[c])
                .unwrap()
        };
        assert_eq!(best((0.35, 0.35)), "hair");
        assert_eq!(best((0.4, 0.8)), "clothes");
        assert_eq!(best((0.05, 0.2)), "background");
    });
}

#[test]
fn puts_the_reflection_at_the_mirror() {
    with_models(|mut ml, photo| {
        let depth = ml.depth(photo, 518).unwrap();
        assert_eq!((depth.width, depth.height, depth.channels), (518, 518, 1));
        // Her back is nearer than her face in the mirror, which is as far as the mirror is.
        let (back, reflection) = (at(&depth, (0.35, 0.4), 0), at(&depth, (0.85, 0.25), 0));
        assert!(back > reflection + 40, "back {back}, reflection {reflection}");
        let (lo, hi) = (depth.data.iter().min().unwrap(), depth.data.iter().max().unwrap());
        assert!(*lo == 0 && *hi == 255, "stretched to the full range: {lo}…{hi}");
    });
}
