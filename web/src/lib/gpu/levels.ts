// SPDX-License-Identifier: Apache-2.0
// Where each CFA colour of a mosaic saturates, measured from the samples: `crates/raw/src/levels.rs`, ported
// so the phone answers what the server answers. A proxy decoded here must be the one the server would have
// sent, so the two must agree bin for bin (docs/MOBILE.md, stage 2); a change to either is a change to both.

/** Bins over the range above `FLOOR` of declared. */
const BINS = 256;
/** Saturation is never looked for below this fraction of the declared maximum. */
const FLOOR = 0.5;
/** A bin must hold this share of the channel, and never fewer than `MIN_SPIKE` samples. */
const SHARE = 0.00002;
const MIN_SPIKE = 256;
/** How many times the bins below it a spike must stand. */
const RELIEF = 8;
/** How much may sit above the spike and still leave it a ceiling, as a share of the spike. */
const OVERSHOOT = 0.1;
/** The answer when nothing qualifies: the sensor reaches what the file declares. */
const DECLARED = 0.995;

/** The top of the range for one channel. `threshold` reads it; see levels.rs for why it is shaped this way. */
export class TopOfRange {
	private bins = new Uint32Array(BINS);
	private counted = 0;
	private total = 0;
	private declared: number;
	private black: number;
	constructor(declared: number, black: number) {
		this.declared = declared;
		this.black = black;
	}

	add(sample: number) {
		const span = this.declared - this.black;
		if (!(span > 0)) return;
		const t = (sample - this.black) / span;
		if (Number.isNaN(t)) return;
		this.total++;
		if (t < FLOOR) return;
		const at = Math.min(BINS - 1, Math.floor(((t - FLOOR) / (1 - FLOOR)) * BINS));
		this.bins[at]++;
		this.counted++;
	}

	threshold(): number {
		if (this.counted === 0) return DECLARED;
		const floorCount = Math.max(this.total * SHARE, MIN_SPIKE);
		let top = -1;
		for (let i = BINS - 1; i >= 0; i--)
			if (this.bins[i] >= floorCount) {
				top = i;
				break;
			}
		if (top <= 0) return DECLARED;
		const from = Math.max(0, top - 8);
		let sum = 0;
		for (let i = from; i < top; i++) sum += this.bins[i];
		const below = sum / Math.max(1, top - from);
		if (this.bins[top] <= below * RELIEF) return DECLARED;
		let above = 0;
		for (let i = top + 1; i < BINS; i++) above += this.bins[i];
		if (above > OVERSHOOT * this.bins[top]) return DECLARED;
		const t = FLOOR + (top / BINS) * (1 - FLOOR);
		return Math.min(DECLARED, Math.max(FLOOR, t));
	}
}

/**
 * Where each CFA colour saturates, as a fraction of `white` above its own black, in colour-index order
 * (0 R, 1 G, 2 B, 3 G2). A colour the pattern never names answers "declared".
 */
export function mosaicClip(
	data: Uint16Array,
	width: number,
	white: number,
	black: number[],
	pattern: number[],
	period: number
): [number, number, number, number] {
	const p = Math.min(6, Math.max(1, period));
	const ch = [0, 1, 2, 3].map((c) => new TopOfRange(white, black[c]));
	if (width === 0) return [DECLARED, DECLARED, DECLARED, DECLARED];
	const rows = Math.floor(data.length / width);
	for (let y = 0; y < rows; y++) {
		const row = (y % p) * 6;
		const base = y * width;
		for (let x = 0; x < width; x++) {
			const c = pattern[row + (x % p)];
			if (c < 4) ch[c].add(data[base + x]);
		}
	}
	return [ch[0].threshold(), ch[1].threshold(), ch[2].threshold(), ch[3].threshold()];
}
