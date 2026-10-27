// SPDX-License-Identifier: Apache-2.0
//! Where a sensor channel actually saturates.
//!
//! Three decode paths used to answer this question three different ways — 99 % of declared, 98.5 %, 98 % of
//! the observed maximum — and all three keyed off a *single* brightest sample, so one stuck pixel at full
//! scale pinned the answer back to the declared level and defeated the heuristic entirely (D6 and D7 in
//! docs/research/pipeline-review-2026-10.md). LibRaw's own `adjust_maximum_thr` has the same weakness.
//!
//! What replaces it: a histogram of the top of the range, per channel. A real saturation level is not one
//! sample, it is a *spike* — every pixel that reached it piles into the same bin, because the sensor could
//! not record anything higher. A stuck pixel is one sample in a bin of its own and never forms a spike.
//!
//! RawTherapee and darktable ship per-camera white-level tables instead. Both are GPL or LGPL data and off
//! limits by CLAUDE.md; a histogram needs no table, and answers for a camera nobody has tabulated.

/// Bins over the range above `FLOOR` of declared. Fine enough that a spike stays in one or two bins, coarse
/// enough that an ordinary photo's noise doesn't look like one.
pub const BINS: usize = 256;

/// Saturation is never looked for below this fraction of the declared maximum. Under it, a "spike" is a
/// subject — a wall, a sky, a studio backdrop — not the sensor running out of range.
const FLOOR: f32 = 0.5;

/// How much of a channel's own counted samples a bin must hold to count as saturation, and how many times
/// the neighbouring bins it must stand above. A clipped sky is percent of the frame; sensor noise spread
/// over the top half of the range is not.
const SHARE: f32 = 0.0005;
const RELIEF: f32 = 8.0;

/// The top of the range for one channel, counted into [`BINS`] bins between `FLOOR` of declared and
/// declared itself. Build one per colour, then ask [`clip_threshold`].
#[derive(Clone, Debug)]
pub struct TopOfRange {
    bins: [u32; BINS],
    counted: u64,
    declared: f32,
    black: f32,
}

impl TopOfRange {
    /// `declared` and `black` are in the sample's own units, as the file states them.
    pub fn new(declared: f32, black: f32) -> Self {
        Self {
            bins: [0; BINS],
            counted: 0,
            declared,
            black,
        }
    }

    /// Count one sample, in the same units as `declared`.
    #[inline]
    pub fn add(&mut self, sample: f32) {
        let span = self.declared - self.black;
        if span <= 0.0 {
            return;
        }
        let t = (sample - self.black) / span;
        // A sample that is not a number counts for nothing; the comparison is written out so that is plain.
        if t.is_nan() || t < FLOOR {
            return;
        }
        let at = ((t - FLOOR) / (1.0 - FLOOR) * BINS as f32) as usize;
        self.bins[at.min(BINS - 1)] += 1;
        self.counted += 1;
    }

    pub fn add_all(&mut self, samples: impl IntoIterator<Item = f32>) {
        for s in samples {
            self.add(s);
        }
    }

    /// Where this channel saturates, as a fraction of the declared maximum above black — the number the
    /// shaders and `LinearImage::rgba` compare a normalized sample against.
    ///
    /// The lowest bin that is a spike: it holds at least [`SHARE`] of what was counted and stands
    /// [`RELIEF`]× above the bins below it. Nothing qualifying means the sensor reaches its declared
    /// maximum, and the answer is 1.0 less a hair for rounding.
    pub fn threshold(&self) -> f32 {
        const DECLARED: f32 = 0.995;
        if self.counted == 0 {
            return DECLARED;
        }
        // Where the data ends: the highest bin holding a real share of the samples. A stuck pixel or two sits
        // above this and is ignored, which is the whole point (D6).
        let floor_count = (self.counted as f32 * SHARE).max(2.0);
        let Some(top) = (0..BINS)
            .rev()
            .find(|&at| self.bins[at] as f32 >= floor_count)
        else {
            return DECLARED;
        };
        // Ending is not the same as saturating. A sensor that saturates piles every blown pixel into its top
        // bin, because it could not record anything higher; a picture that simply has no bright highlights
        // tapers off instead, and its brightest pixels are not clipped. Only a pile-up is saturation.
        let below = {
            let from = top.saturating_sub(8);
            let n = (top - from).max(1);
            self.bins[from..top].iter().map(|&b| b as f32).sum::<f32>() / n as f32
        };
        if top == 0 || self.bins[top] as f32 <= below * RELIEF {
            return DECLARED;
        }
        // The spike's lower edge: samples that reached it are saturated.
        let t = FLOOR + (top as f32 / BINS as f32) * (1.0 - FLOOR);
        t.clamp(FLOOR, DECLARED)
    }
}

/// The four CFA colours' thresholds (0 R, 1 G, 2 B, 3 G2), as fractions of declared above black.
pub fn clip_threshold(channels: &[TopOfRange; 4]) -> [f32; 4] {
    std::array::from_fn(|c| channels[c].threshold())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that saturates where the file says it does: no spike short of the top, so the threshold
    /// stays at the declared maximum.
    #[test]
    fn a_sensor_that_reaches_its_declared_maximum() {
        let mut t = TopOfRange::new(16383.0, 512.0);
        // A smooth ramp over the whole range, as an ordinary picture gives.
        for i in 0..100_000 {
            t.add(512.0 + (i as f32 / 100_000.0) * (16383.0 - 512.0));
        }
        assert!(t.threshold() > 0.98, "got {}", t.threshold());
    }

    /// A sensor that saturates below its declared maximum: every clipped pixel piles into one bin, and that
    /// spike is the real white level.
    #[test]
    fn a_spike_below_declared_is_the_real_saturation() {
        let mut t = TopOfRange::new(16383.0, 512.0);
        for i in 0..100_000 {
            t.add(512.0 + (i as f32 / 100_000.0) * (15000.0 - 512.0));
        }
        // 3 % of the frame blown, all landing at 15000 because the sensor cannot record higher.
        for _ in 0..3_000 {
            t.add(15000.0);
        }
        let got = t.threshold();
        let real = (15000.0 - 512.0) / (16383.0 - 512.0);
        assert!(
            (got - real).abs() < 0.03,
            "threshold {got} should be near {real}"
        );
    }

    /// The dangerous mistake in the other direction: a frame whose brightest pixel is well short of the top
    /// has no clipping at all, and must not have its highlights declared saturated just because the data
    /// ends there. Ending is not saturating — only a pile-up is.
    #[test]
    fn a_frame_with_no_bright_highlights_is_not_clipped() {
        let mut t = TopOfRange::new(16383.0, 512.0);
        for i in 0..100_000 {
            t.add(512.0 + (i as f32 / 100_000.0) * (11000.0 - 512.0));
        }
        assert!(t.threshold() > 0.99, "got {}", t.threshold());
    }

    /// D6, the point of all this: one stuck pixel at full scale used to pin the threshold back to the
    /// declared maximum and hide the real one. It is a single sample, so it never forms a spike.
    #[test]
    fn one_stuck_pixel_does_not_move_the_threshold() {
        let build = |stuck: usize| {
            let mut t = TopOfRange::new(16383.0, 512.0);
            for i in 0..100_000 {
                t.add(512.0 + (i as f32 / 100_000.0) * (15000.0 - 512.0));
            }
            for _ in 0..3_000 {
                t.add(15000.0);
            }
            for _ in 0..stuck {
                t.add(16383.0);
            }
            t.threshold()
        };
        assert_eq!(build(0), build(1), "a stuck pixel changed the answer");
        assert_eq!(build(0), build(5));
    }

    /// A channel nothing was counted into — a frame with no highlights at all — answers "declared" rather
    /// than dividing by zero or picking bin 0.
    #[test]
    fn an_empty_channel_falls_back_to_declared() {
        let t = TopOfRange::new(16383.0, 512.0);
        assert!(t.threshold() > 0.99);
        // Levels that make no sense count nothing rather than panicking; `RawFile::mosaic` refuses these
        // first (D4), so this is belt and braces.
        let mut bad = TopOfRange::new(100.0, 100.0);
        bad.add_all([50.0, 100.0, 200.0]);
        assert!(bad.threshold() > 0.99);
    }

    /// Channels are answered separately: blue clipping first is the usual case under tungsten, and it must
    /// not drag red and green down with it.
    #[test]
    fn channels_are_answered_one_by_one() {
        let ramp = |top: f32| {
            let mut t = TopOfRange::new(16383.0, 512.0);
            for i in 0..100_000 {
                t.add(512.0 + (i as f32 / 100_000.0) * (top - 512.0));
            }
            for _ in 0..3_000 {
                t.add(top);
            }
            t
        };
        let got = clip_threshold(&[ramp(16383.0), ramp(16383.0), ramp(12000.0), ramp(16383.0)]);
        assert!(got[2] < got[0] - 0.1, "blue {} vs red {}", got[2], got[0]);
    }
}
