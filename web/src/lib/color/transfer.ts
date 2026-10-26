// SPDX-License-Identifier: Apache-2.0
// The sRGB transfer curve and IEEE half floats, for the few places the page itself touches pixel values. The
// shaders have their own (shaders/common/transfer.wgsl); this is the one copy on the TypeScript side.

/** sRGB (and Display P3, which shares the curve) encoded → linear. */
export const srgbDecode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

/** Linear → sRGB encoded. */
export const srgbEncode = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);

/** An IEEE half float's bits → its value. */
export function fromHalf(h: number): number {
	const exp = (h >> 10) & 0x1f;
	const frac = h & 0x3ff;
	const sign = h & 0x8000 ? -1 : 1;
	if (exp === 0) return sign * frac * 2 ** -24;
	if (exp === 31) return frac ? NaN : sign * Infinity;
	return sign * (1 + frac / 1024) * 2 ** (exp - 15);
}

const f32 = new Float32Array(1);
const u32 = new Uint32Array(f32.buffer);

/** A value → the nearest IEEE half float's bits (round to nearest even; overflow to infinity). */
export function toHalf(v: number): number {
	f32[0] = v;
	const x = u32[0];
	const sign = (x >>> 16) & 0x8000;
	const exp = (x >>> 23) & 0xff;
	let mant = x & 0x7fffff;
	if (exp === 0xff) return sign | 0x7c00 | (mant ? 0x200 : 0);
	let e = exp - 127 + 15;
	if (e >= 31) return sign | 0x7c00;
	if (e <= 0) {
		if (e < -10) return sign;
		mant |= 0x800000;
		const shift = 14 - e;
		const half = mant >>> shift;
		const rest = mant & ((1 << shift) - 1);
		const mid = 1 << (shift - 1);
		return sign | (half + (rest > mid || (rest === mid && half & 1) ? 1 : 0));
	}
	const half = (e << 10) | (mant >>> 13);
	const rest = mant & 0x1fff;
	return sign | (half + (rest > 0x1000 || (rest === 0x1000 && half & 1) ? 1 : 0));
}
