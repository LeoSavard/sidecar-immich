// SPDX-License-Identifier: Apache-2.0
// A developed photo (JPEG, PNG, WebP, AVIF, and HEIC where the browser has a decoder for it) decoded by the
// phone itself, into the proxy shape the develop pass takes from the server: linear RGBA16F, alpha 0, long
// edge bounded. This is stage 0 of the plan in docs/research/pipeline-review-2026-10.md §7.
//
// The browser's own decoder does the work, which is why HEIC is fine here: on an iPhone it is the system's
// decoder, not a library Sidecar ships (the server's licence boundary, CLAUDE.md, never comes into it). The
// browser also applies the file's orientation and converts whatever profile it carries — Adobe RGB, ProPhoto —
// into the canvas's colour space, which the server's decoder doesn't yet do (the review's D10).
import { srgbDecode, toHalf } from '$lib/color/transfer';
import type { DisplaySpace } from '$lib/types';

export type LocalPhoto = {
	width: number;
	height: number;
	/** RGBA16F, row-major, top-left origin: what `DevelopPass.setSource` takes. */
	f16: Uint16Array;
	/** The primaries `f16` is in: Display P3 where the browser can draw into a P3 canvas, sRGB otherwise. */
	space: DisplaySpace;
};

/** Encoded 8-bit value → linear half float, the whole curve at once: one lookup per sample. */
const LINEAR = Uint16Array.from({ length: 256 }, (_, i) => toHalf(srgbDecode(i / 255)));

/**
 * Decode `file` at no more than `longEdge` pixels on its long side, or `null` when this browser can't decode
 * it at all. The caller decides what a RAW is before asking: some browsers will decode a RAW's embedded JPEG
 * and hand that back as if it were the photo.
 */
export async function decodeLocally(file: Blob, longEdge: number): Promise<LocalPhoto | null> {
	let bitmap: ImageBitmap;
	try {
		bitmap = await createImageBitmap(file, { imageOrientation: 'from-image' });
	} catch {
		return null;
	}
	const scale = Math.min(1, longEdge / Math.max(bitmap.width, bitmap.height));
	const width = Math.max(1, Math.round(bitmap.width * scale));
	const height = Math.max(1, Math.round(bitmap.height * scale));

	// Display P3 holds sRGB and most of what a phone records. A browser without P3 canvases ignores the
	// request and hands back sRGB, and the ImageData says which it got.
	const ctx = new OffscreenCanvas(width, height).getContext('2d', { colorSpace: 'display-p3', willReadFrequently: true });
	if (!ctx) {
		bitmap.close();
		return null;
	}
	// The scaling averages encoded values rather than light, which darkens fine high-contrast detail a little.
	// A proxy can carry that; an export from the phone (stage 3) should decode at full size instead.
	ctx.imageSmoothingQuality = 'high';
	ctx.drawImage(bitmap, 0, 0, width, height);
	bitmap.close();
	const image = ctx.getImageData(0, 0, width, height, { colorSpace: 'display-p3' });
	const space: DisplaySpace = image.colorSpace === 'display-p3' ? 'display-p3' : 'srgb';
	const pixels = image.data;

	const f16 = new Uint16Array(width * height * 4);
	for (let i = 0; i < pixels.length; i += 4) {
		f16[i] = LINEAR[pixels[i]];
		f16[i + 1] = LINEAR[pixels[i + 1]];
		f16[i + 2] = LINEAR[pixels[i + 2]];
		// Alpha is clip coverage downstream, which only a RAW has (crates/raw/src/decoded.rs says why).
		f16[i + 3] = 0;
	}
	return { width, height, f16, space };
}
