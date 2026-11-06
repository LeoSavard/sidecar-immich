// SPDX-License-Identifier: Apache-2.0
/**
 * Focus peaking, refined (DESIGN.md §11, "Focus peaking"): the focus measure weighted by the subject mask, so
 * peaking answers "is the subject sharp?" rather than "where is anything sharp?", and each face's eyes rated from
 * the same measure, largest face first — what a portrait is judged by.
 */
import type { Face } from '$lib/masks';

export type Grey = { width: number; height: number; bytes: Uint8Array };

/** `view.wgsl`'s threshold: a focus-mask value at or above it is in focus. */
export const IN_FOCUS = 0.5;

/** A greyscale PNG's values, one byte a pixel. */
export async function decodeGrey(png: Blob | ArrayBuffer): Promise<Grey> {
	const bmp = await createImageBitmap(png instanceof Blob ? png : new Blob([png]));
	const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d')!;
	c.drawImage(bmp, 0, 0);
	const d = c.getImageData(0, 0, bmp.width, bmp.height).data;
	const bytes = new Uint8Array(bmp.width * bmp.height);
	for (let k = 0; k < bytes.length; k++) bytes[k] = d[k * 4];
	return { width: bmp.width, height: bmp.height, bytes };
}

/** `focus` × `subject`, at the focus mask's size (the subject mask read at its nearest pixel). */
export function weighByMask(focus: Grey, subject: Grey): Grey {
	const out = new Uint8Array(focus.bytes.length);
	for (let y = 0; y < focus.height; y++) {
		const sy = Math.min(subject.height - 1, Math.floor(((y + 0.5) * subject.height) / focus.height));
		for (let x = 0; x < focus.width; x++) {
			const sx = Math.min(subject.width - 1, Math.floor(((x + 0.5) * subject.width) / focus.width));
			const i = y * focus.width + x;
			out[i] = Math.round((focus.bytes[i] * subject.bytes[sy * subject.width + sx]) / 255);
		}
	}
	return { width: focus.width, height: focus.height, bytes: out };
}

export type Eye = { face: number; side: 'right' | 'left'; at: [number, number]; score: number; sharp: boolean };

/**
 * Each face's eyes, rated by the focus measure around them: the highest value within 15 % of the face's width of
 * the eye's landmark. Largest face first, and within a face its sharper eye first.
 */
export function rateEyes(focus: Grey, faces: Face[]): Eye[] {
	const order = faces.map((f, i) => ({ f, i })).sort((a, b) => b.f.bounds[2] * b.f.bounds[3] - a.f.bounds[2] * a.f.bounds[3]);
	const out: Eye[] = [];
	for (const { f, i } of order) {
		const eyes = (['right', 'left'] as const).map((side, k) => {
			const [ex, ey] = f.landmarks[k];
			const r = Math.max(1, Math.round(0.15 * f.bounds[2] * focus.width));
			const [cx, cy] = [Math.round(ex * focus.width), Math.round(ey * focus.height)];
			let best = 0;
			for (let y = Math.max(0, cy - r); y <= Math.min(focus.height - 1, cy + r); y++)
				for (let x = Math.max(0, cx - r); x <= Math.min(focus.width - 1, cx + r); x++)
					if ((x - cx) ** 2 + (y - cy) ** 2 <= r * r) best = Math.max(best, focus.bytes[y * focus.width + x]);
			const score = best / 255;
			return { face: i, side, at: [ex, ey] as [number, number], score, sharp: score >= IN_FOCUS };
		});
		out.push(...eyes.sort((a, b) => b.score - a.score));
	}
	return out;
}
