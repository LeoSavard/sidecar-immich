// SPDX-License-Identifier: Apache-2.0
//! What the developer and the development do to a black-and-white negative (DESIGN.md §7, "Developers").
//!
//! Three effects, from the photographic literature rather than any one formula's datasheet:
//! - **Adjacency.** Developer exhausts where the negative is dense and stays fresh where it is thin, and both
//!   diffuse a fraction of a millimetre: next to an edge the dense side develops further and the thin side
//!   less (the Eberhard effect, Mackie lines). In log exposure that is `x + a·(x − blur(x))`, with the blur's
//!   reach in millimetres on the negative. The blurred exposure comes from a low-resolution map of the whole
//!   frame (`MAP_LONG`), which the develop pass reads like a mask.
//! - **Compensation.** A dilute or exhausted developer slows down in the highlights: above a knee the density
//!   grows more slowly, holding highlights a print would otherwise lose.
//! - **Development.** Longer or shorter development (the zone system's N+ and N−) raises or lowers the
//!   negative's contrast above its speed point in the shadows; shadows barely move.
//!
//! Each developer is a mix of the first two, with what it does to grain: solvent developers shrink it, sharp
//! ones show it; a staining developer's stain sits on the dense parts and hides their grain.

/// A developer's effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Developer {
    /// Adjacency: how far a pixel's log exposure moves from its surroundings', as a share of the difference.
    pub adjacency: f64,
    /// How far the developer diffuses, in millimetres on a 35 mm negative: the adjacency blur's standard deviation.
    pub reach_mm: f64,
    /// Compensation: how much less density grows above the knee (0 none, 1 none at all).
    pub compensation: f64,
    /// Grain, as a factor on the grain's strength.
    pub grain: f64,
    /// A staining developer's grain masking: how much of the grain the stain hides at the white point.
    pub stain: f64,
}

/// The developers by id, as `Monochrome.developer` names them. An id this version doesn't know is the standard
/// developer: the negative still develops, as any developer would develop it.
pub fn developer(id: &str) -> Developer {
    let d = |adjacency, reach_mm, compensation, grain, stain| Developer {
        adjacency,
        reach_mm,
        compensation,
        grain,
        stain,
    };
    match id {
        // A solvent fine-grain developer: finer grain, no edge effects.
        "fine-grain" => d(0.0, 0.1, 0.0, 0.75, 0.0),
        // A dilute high-acutance developer: strong edges over a short reach, grain shown off.
        "acutance" => d(0.35, 0.08, 0.1, 1.2, 0.0),
        // A compensating developer (two-bath, highly dilute or stand development): highlights held, broad edges.
        "compensating" => d(0.25, 0.3, 0.45, 1.0, 0.0),
        // A staining developer (pyro's kind): the stain compensates and hides the highlights' grain; tanning
        // sharpens edges a little.
        "staining" => d(0.2, 0.15, 0.3, 1.0, 0.5),
        // A standard general-purpose developer: none of it.
        _ => d(0.0, 0.1, 0.0, 1.0, 0.0),
    }
}

/// Development N−2 … N+2 → the negative's contrast above its speed point, as a factor: the zone system's N+1
/// fits four zones of the subject into five of the negative.
pub fn gamma(n: f64) -> f64 {
    5.0 / (5.0 - n.clamp(-2.0, 2.0))
}

/// Development N → a factor on the grain: longer development grows it, shorter shrinks it.
pub fn development_grain(n: f64) -> f64 {
    1.0 + 0.15 * n.clamp(-2.0, 2.0)
}

/// The long edge of the adjacency map, at most: it is averaged down from the frame by a whole number.
pub const MAP_LONG: u32 = 1024;

/// How many of the frame's pixels a map texel averages in each direction, for a frame `frame` pixels across.
pub fn map_scale(frame: [u32; 2]) -> u32 {
    frame[0].max(frame[1]).div_ceil(MAP_LONG).max(1)
}

/// The adjacency map's size for a frame `frame` pixels across.
pub fn map_size(frame: [u32; 2]) -> [u32; 2] {
    let k = map_scale(frame);
    frame.map(|v| v.div_ceil(k).max(1))
}

/// The map's blur in its own texels, from `DevelopParams.developer`'s reach (a share of the long edge), for a
/// frame `frame` pixels across.
pub fn map_sigma(reach: f32, frame: [u32; 2]) -> f32 {
    reach * frame[0].max(frame[1]) as f32 / map_scale(frame) as f32
}
