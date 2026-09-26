// SPDX-License-Identifier: Apache-2.0
// Slider readouts. A value shows `digits` decimals, or as many more (up to three) as it actually carries — so
// a fine-mode edit of 12.3 reads 12.3, and an ordinary one still reads 12.

export function decimals(v: number, digits = 0): string {
	const carried = (String(Number(v.toFixed(3))).split('.')[1] ?? '').length;
	return v.toFixed(Math.max(digits, carried));
}

export const signed = (v: number, digits = 0) => `${v > 0 ? '+' : ''}${decimals(v, digits)}`;

/** A slider position derived from a stored parameter, without the float noise of the conversion. */
export const clean = (v: number) => Number(v.toFixed(3));
