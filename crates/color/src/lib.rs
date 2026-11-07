// SPDX-License-Identifier: Apache-2.0
//! Colour maths shared by the worker and the browser (via wasm32). Everything the develop shaders need that
//! is cheaper to compute once per edit than once per pixel lives here.

pub mod camera;
pub mod cct;
pub mod depth;
pub mod developer;
mod film_data;
pub mod geometry;
pub mod icc;
pub mod lens;
pub mod mat;
pub mod params;
pub mod slide;
pub mod spectral;
mod spectral_data;
pub mod spline;
pub mod stock;
pub mod tone;

pub use params::{DevelopParams, DisplaySpace, EditParams, Packed, SourceKind, pack};

#[cfg(test)]
mod uniforms {
    use sidecar_uniform::Requirement as R;

    /// `shaders/uniforms/` holds this crate's uniform structs as WGSL, generated from them, and naga lays
    /// each one out as Rust does. `npm run uniforms` in `web/` rewrites the files after a change.
    #[test]
    fn uniforms() {
        sidecar_uniform::check::uniforms(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shaders"),
            &[
                R::of::<crate::params::DevelopParams>(),
                R::of::<crate::params::MaskLayer>(),
                R::of::<crate::params::MaskParams>(),
                R::of::<crate::params::EdgeParams>(),
                R::of::<crate::params::GradientParams>(),
                R::of::<crate::depth::DepthParams>(),
                R::of::<crate::depth::DepthRefineParams>(),
                R::of::<crate::geometry::WarpParams>(),
                R::of::<crate::lens::LensParams>(),
            ],
        );
    }
    /// The browser reads two fields of the develop uniform by float offset, and sizes it, by hand: the
    /// numbers in `develop.ts` and `pack.ts` are Rust's, or this fails.
    #[test]
    fn the_browsers_develop_offsets_are_rusts() {
        use crate::params::DevelopParams;
        let number = |src: &str, name: &str| -> usize {
            let at = src.find(&format!("const {name} = ")).unwrap() + name.len() + 9;
            src[at..].split(';').next().unwrap().trim().parse().unwrap()
        };
        let develop = include_str!("../../../web/src/lib/gpu/develop.ts");
        let pack = include_str!("../../../web/src/lib/color/pack.ts");
        assert_eq!(
            number(develop, "MONO") * 4,
            std::mem::offset_of!(DevelopParams, mono)
        );
        assert_eq!(
            number(develop, "DEVELOPER") * 4,
            std::mem::offset_of!(DevelopParams, developer)
        );
        assert_eq!(
            number(pack, "DEVELOP_UNIFORM_BYTES"),
            std::mem::size_of::<DevelopParams>()
        );
    }
}
