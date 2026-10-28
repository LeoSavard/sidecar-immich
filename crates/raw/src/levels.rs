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

/// How much of the **channel** a bin must hold to count as saturation, and how many times the neighbouring
/// bins it must stand above. A clipped sky is percent of the frame; sensor noise spread over the top half of
/// the range is not.
///
/// A share of the whole channel, not of the samples above `FLOOR`: on a dark frame almost nothing reaches
/// the top of the range, and a share of *that* is a share of a handful. A 4-megapixel channel of an Olympus
/// E-PL5 frame shot at dusk had five samples above half scale, and two of them in one bin read as a sensor
/// running out of range at 54.88 %.
const SHARE: f32 = 0.00002;
/// And never fewer than this many samples, however small the frame: saturation is a region of a picture.
/// 256 samples of one colour is a thousand pixels, a patch some thirty across — a small blown specular, and
/// the least that is worth acting on. A dark frame's whole top-of-range population is a handful, so this
/// alone ends the mistake that [`SHARE`] describes; the bar is low because the shape tests below are what
/// actually tell saturation from subject, and a bar high enough to matter threw away real ceilings. On one
/// Canon EOS M3 frame blue piled 730 samples into a single bin, standing 24× above its neighbours with
/// nothing whatever above it, and a bar of 0.05 % of the channel called that noise.
const MIN_SPIKE: f32 = 256.0;
const RELIEF: f32 = 8.0;
/// How much may sit above the spike and still leave it saturation, as a share of the spike itself. A stuck
/// pixel or a few is the point of all this (D6); a real population above it means the sensor goes higher and
/// whatever the bin holds is subject, not a ceiling.
const OVERSHOOT: f32 = 0.1;

/// The top of the range for one channel, counted into [`BINS`] bins between `FLOOR` of declared and
/// declared itself. Build one per colour, then ask [`clip_threshold`].
#[derive(Clone, Debug)]
pub struct TopOfRange {
    bins: [u32; BINS],
    counted: u64,
    total: u64,
    declared: f32,
    black: f32,
}

impl TopOfRange {
    /// `declared` and `black` are in the sample's own units, as the file states them.
    pub fn new(declared: f32, black: f32) -> Self {
        Self {
            bins: [0; BINS],
            counted: 0,
            total: 0,
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
        if t.is_nan() {
            return;
        }
        self.total += 1;
        if t < FLOOR {
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

    /// The bins themselves, lowest first, with how many samples reached them and how many the channel holds
    /// altogether, for diagnostics (`sidecar-dev levels --bins`).
    pub fn bins(&self) -> (&[u32; BINS], u64, u64) {
        (&self.bins, self.counted, self.total)
    }

    /// Where this channel saturates, as a fraction of the declared maximum above black — the number the
    /// shaders and `LinearImage::rgba` compare a normalized sample against.
    ///
    /// The highest bin that is a ceiling: it holds at least [`SHARE`] of the channel (and [`MIN_SPIKE`]
    /// samples), stands [`RELIEF`]× above the bins below it, and has next to nothing above it
    /// ([`OVERSHOOT`]). Nothing qualifying means the sensor reaches its declared maximum, and the answer is
    /// 1.0 less a hair for rounding — deliberately the safe way round, since declaring saturation where
    /// there is none hands good pixels to highlight reconstruction, while missing a small blown patch only
    /// leaves it as the sensor recorded it.
    pub fn threshold(&self) -> f32 {
        const DECLARED: f32 = 0.995;
        if self.counted == 0 {
            return DECLARED;
        }
        // Where the data ends: the highest bin holding a real share of the channel. A stuck pixel or two sits
        // above this and is ignored, which is the whole point (D6).
        let floor_count = (self.total as f32 * SHARE).max(MIN_SPIKE);
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
        // And a ceiling has nothing above it. Samples that got past the spike are the stuck pixels D6 is
        // about, so a few are expected; a population of them says the sensor was never at its limit here.
        let above: u64 = self.bins[top + 1..].iter().map(|&b| b as u64).sum();
        if above as f32 > OVERSHOOT * self.bins[top] as f32 {
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

/// Where each CFA colour of a mosaic saturates, as fractions of the declared maximum above that colour's
/// black — the four numbers `normalize.wgsl` and the highlight passes compare a normalized sample against.
///
/// `pattern[y % period][x % period]` names the colour, as `Mosaic` states it. A colour the pattern never
/// names is counted into nothing and answers "declared", so a Bayer file's unused G2 lane is harmless.
pub fn mosaic_clip(
    data: &[u16],
    width: u32,
    white: f32,
    black: &[f32; 4],
    pattern: &[[u8; 6]; 6],
    period: u32,
) -> [f32; 4] {
    clip_threshold(&mosaic_histograms(
        data, width, white, black, pattern, period,
    ))
}

/// The four histograms [`mosaic_clip`] asks, for a caller that wants to look at them.
pub fn mosaic_histograms(
    data: &[u16],
    width: u32,
    white: f32,
    black: &[f32; 4],
    pattern: &[[u8; 6]; 6],
    period: u32,
) -> [TopOfRange; 4] {
    let period = (period as usize).clamp(1, 6);
    let w = width as usize;
    let mut ch = std::array::from_fn(|c| TopOfRange::new(white, black[c]));
    if w == 0 {
        return ch;
    }
    for (y, row) in data.chunks(w).enumerate() {
        let colours = &pattern[y % period];
        for (x, &s) in row.iter().enumerate() {
            let c = colours[x % period] as usize;
            if let Some(t) = ch.get_mut(c) {
                t.add(s as f32);
            }
        }
    }
    ch
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that saturates where the file says it does: no spike short of the top, so the threshold
    /// stays at the declared maximum.
    #[test]
    fn a_sensor_that_reaches_its_declared_maximum() {
        let mut t = TopOfRange::new(16383.0, 512.0);
        // A smooth ramp over the whole range, as an ordinary picture gives. Big enough that every bin clears
        // the floor, so what rejects it is the shape — no bin stands above its neighbours — and not a count.
        for i in 0..1_000_000 {
            t.add(512.0 + (i as f32 / 1_000_000.0) * (16383.0 - 512.0));
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

    /// The detector's own failure mode, found by running it over 222 frames from the owner's library: a dark
    /// frame has almost nothing above `FLOOR`, so a share of what *is* up there is a share of a handful. One
    /// Olympus E-PL5 frame had five samples above half scale in a four-megapixel channel, two of them in one
    /// bin, and that read as the sensor running out of range at 54.88 % — which would have handed a third of
    /// the picture to highlight reconstruction. Saturation is a region of a photograph, not two pixels.
    #[test]
    fn a_handful_of_samples_at_the_top_of_a_dark_frame_is_not_saturation() {
        let mut t = TopOfRange::new(4043.0, 255.0);
        for _ in 0..4_000_000 {
            t.add(600.0);
        }
        for s in [2300.0, 2300.0, 2600.0, 3400.0, 4042.0] {
            t.add(s);
        }
        assert!(t.threshold() > 0.99, "got {}", t.threshold());
    }

    /// A spike with a real population above it is not a ceiling: the sensor plainly records higher, so the
    /// bin is a bright subject and the channel is not clipped there.
    #[test]
    fn a_bin_with_plenty_above_it_is_not_a_ceiling() {
        let span = 16383.0 - 512.0;
        let mut t = TopOfRange::new(16383.0, 512.0);
        // A picture reaching 60 % of the range, with a flat subject piled up at the top of it.
        for i in 0..200_000 {
            t.add(512.0 + (i as f32 / 200_000.0) * 0.6 * span);
        }
        for _ in 0..20_000 {
            t.add(512.0 + 0.6 * span);
        }
        // And a thin tail carrying on above — too thin for any bin of its own to clear the floor, but far
        // more than a stuck pixel or two in total. The pile-up is a subject, not a limit.
        for i in 0..4_000 {
            t.add(512.0 + (0.6 + 0.4 * (i as f32 / 4_000.0)) * span);
        }
        assert!(t.threshold() > 0.99, "got {}", t.threshold());
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

    /// A mosaic, walked as the decode hands it over: a Bayer frame whose blue saturates at 12,000 while red
    /// and green reach the declared maximum. The pattern never names G2, so that lane answers "declared"
    /// rather than zero — the lane LibRaw leaves uninitialized on a Bayer file.
    #[test]
    fn a_bayer_mosaic_is_measured_per_colour() {
        // RGGB: colour 0 at (0,0), 1 at (0,1) and (1,0), 2 at (1,1).
        let mut pattern = [[0u8; 6]; 6];
        pattern[0][1] = 1;
        pattern[1][0] = 1;
        pattern[1][1] = 2;
        let (w, h) = (256usize, 256usize);
        let mut data = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let top = if pattern[y % 2][x % 2] == 2 {
                    12000.0
                } else {
                    16383.0
                };
                // A ramp across the row, with the last eighth of it pinned at the colour's saturation.
                let t = (x as f32 / w as f32 * 1.14).min(1.0);
                data[y * w + x] = (512.0 + t * (top - 512.0)) as u16;
            }
        }
        let got = mosaic_clip(&data, w as u32, 16383.0, &[512.0; 4], &pattern, 2);
        let blue = (12000.0 - 512.0) / (16383.0 - 512.0);
        assert!(got[0] > 0.98, "red {}", got[0]);
        assert!(got[1] > 0.98, "green {}", got[1]);
        assert!(
            (got[2] - blue).abs() < 0.03,
            "blue {} should be near {blue}",
            got[2]
        );
        assert!(got[3] > 0.98, "unused G2 {}", got[3]);
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
