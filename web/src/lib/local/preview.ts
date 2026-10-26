// SPDX-License-Identifier: Apache-2.0
// The JPEG a camera embeds in its RAW file: a full-size or near-full-size rendering of the photo, there to be
// shown before anything is decoded. Stage 1 of docs/MOBILE.md: the phone shows it the moment a RAW is chosen,
// with nothing uploaded and no RAW decoder involved.
//
// Every RAW format Sidecar opens stores it as a plain JPEG somewhere in the file — in a TIFF IFD (NEF, CR2,
// ARW, DNG, PEF), as a tag's value (RW2), behind a fixed header (RAF), in an ISO BMFF box (CR3) — so instead of
// one parser per format this finds JPEG streams by their own structure. Each candidate is walked marker
// segment by marker segment, which steps over the thumbnail nested inside a preview's EXIF, and is kept only if
// it is a Huffman-coded picture: lossless JPEG (SOF3), which CR2 and DNG use for the sensor data itself, is
// not a preview. The largest one wins. Written from the JPEG (ITU T.81) and TIFF 6.0 specifications.
//
// No imports, so `node --experimental-strip-types` can load it for scripts/preview-check.mjs.

export type EmbeddedJpeg = {
	/** Byte range of the JPEG stream within the file. */
	start: number;
	end: number;
	width: number;
	height: number;
	/** EXIF orientation (1–8) the JPEG states itself, when it has an EXIF block that says. */
	orientation?: number;
};

/** Frame types that code a picture with Huffman tables: baseline, extended, progressive. */
const PICTURE_FRAMES = new Set([0xc0, 0xc1, 0xc2]);
/** Every start-of-frame marker, so a frame type outside the set above is recognised and refused. */
const FRAMES = new Set([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf]);
/** Smaller than this is a thumbnail, not something to show a photo with. */
const MIN_EDGE = 160;

/** The largest embedded picture in `bytes`, or null when the file carries none. */
export function largestEmbeddedJpeg(bytes: Uint8Array): EmbeddedJpeg | null {
	let best: EmbeddedJpeg | null = null;
	let at = 0;
	while (at < bytes.length - 3) {
		const soi = indexOfSoi(bytes, at);
		if (soi < 0) break;
		const walked = walkJpeg(bytes, soi);
		const found = walked?.picture;
		if (found && (!best || found.width * found.height > best.width * best.height)) best = found;
		// A whole stream's own bytes hold no other preview worth having (a picture's EXIF thumbnail is smaller,
		// and lossless sensor data is megabytes to skip), so carry on after it; after a false start, one byte on.
		at = walked ? walked.end : soi + 1;
	}
	return best;
}

function indexOfSoi(b: Uint8Array, from: number): number {
	for (let i = b.indexOf(0xff, from); i >= 0 && i < b.length - 2; i = b.indexOf(0xff, i + 1)) {
		if (b[i + 1] === 0xd8 && b[i + 2] === 0xff) return i;
	}
	return -1;
}

const u16be = (b: Uint8Array, i: number) => (b[i] << 8) | b[i + 1];

/**
 * The JPEG starting at `soi`, if one really does: where it ends, and the picture when it is one (rather than,
 * say, a RAW's lossless sensor data). Null for a false start.
 */
function walkJpeg(b: Uint8Array, soi: number): { end: number; picture: EmbeddedJpeg | null } | null {
	let p = soi + 2;
	let frame = -1;
	let width = 0;
	let height = 0;
	let components = 0;
	let orientation: number | undefined;
	let scanned = false;
	for (let segments = 0; segments < 1000; segments++) {
		// Fill bytes (0xFF) may pad before a marker.
		if (b[p] !== 0xff) return null;
		while (b[p] === 0xff && p < b.length) p++;
		if (p >= b.length) return null;
		const marker = b[p++];
		if (marker === 0xd9) {
			// End of image: a picture only if a frame and at least one scan came before it.
			if (!scanned) return null;
			const picture = PICTURE_FRAMES.has(frame) && (components === 1 || components === 3) && width >= MIN_EDGE && height >= MIN_EDGE;
			return { end: p, picture: picture ? { start: soi, end: p, width, height, orientation } : null };
		}
		if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue; // standalone
		if (marker === 0xd8 || marker === 0x00) return null;
		if (p + 2 > b.length) return null;
		const length = u16be(b, p);
		if (length < 2 || p + length > b.length) return null;
		const body = p + 2;
		if (FRAMES.has(marker)) {
			if (frame >= 0) return null; // one frame per image
			frame = marker;
			height = u16be(b, body + 1);
			width = u16be(b, body + 3);
			components = b[body + 5];
		} else if (marker === 0xe1 && orientation === undefined && isExif(b, body)) {
			orientation = tiffOrientation(b, body + 6, body + length - 2);
		}
		p += length;
		if (marker === 0xda) {
			// Entropy-coded data runs to the next marker that isn't a stuffed 0xFF00 or a restart marker.
			if (frame < 0) return null;
			scanned = true;
			for (;;) {
				const ff = b.indexOf(0xff, p);
				if (ff < 0 || ff + 1 >= b.length) return null;
				const next = b[ff + 1];
				if (next === 0x00 || (next >= 0xd0 && next <= 0xd7) || next === 0xff) {
					p = ff + (next === 0xff ? 1 : 2);
					continue;
				}
				p = ff;
				break;
			}
		}
	}
	return null;
}

function isExif(b: Uint8Array, at: number): boolean {
	return b[at] === 0x45 && b[at + 1] === 0x78 && b[at + 2] === 0x69 && b[at + 3] === 0x66 && b[at + 4] === 0 && b[at + 5] === 0;
}

/**
 * Orientation (tag 0x0112) from the first IFD of a TIFF structure at `tiff`. Accepts the TIFF magic and the
 * variants RAW formats put there: 42, Olympus ORF's 0x4F52 / 0x5352, Panasonic RW2's 0x55.
 */
export function tiffOrientation(b: Uint8Array, tiff: number, limit = b.length): number | undefined {
	if (tiff + 8 > limit) return undefined;
	const le = b[tiff] === 0x49 && b[tiff + 1] === 0x49;
	if (!le && !(b[tiff] === 0x4d && b[tiff + 1] === 0x4d)) return undefined;
	const u16 = (i: number) => (le ? b[i] | (b[i + 1] << 8) : (b[i] << 8) | b[i + 1]);
	const u32 = (i: number) => (le ? (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16)) + b[i + 3] * 2 ** 24 : b[i] * 2 ** 24 + ((b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]));
	if (![42, 0x4f52, 0x5352, 0x55].includes(u16(tiff + 2))) return undefined;
	const ifd = tiff + u32(tiff + 4);
	if (ifd + 2 > limit) return undefined;
	const count = Math.min(u16(ifd), 1000);
	for (let i = 0; i < count; i++) {
		const e = ifd + 2 + 12 * i;
		if (e + 12 > limit) return undefined;
		if (u16(e) === 0x0112) {
			const v = u16(e + 8);
			return v >= 1 && v <= 8 ? v : undefined;
		}
	}
	return undefined;
}

/**
 * The RAW file's own orientation: its TIFF header's first IFD for the TIFF-based formats, or the TIFF block in
 * a CR3's `CMT1` box. Fuji's RAF has neither here, and its embedded JPEG carries its own EXIF instead.
 */
export function rawOrientation(b: Uint8Array): number | undefined {
	const direct = tiffOrientation(b, 0);
	if (direct !== undefined) return direct;
	// CR3 (ISO BMFF): the box type is followed directly by its TIFF data. It sits in the header, near the start.
	const head = b.subarray(0, Math.min(b.length, 1 << 16));
	for (let i = head.indexOf(0x43); i >= 0 && i < head.length - 4; i = head.indexOf(0x43, i + 1)) {
		if (head[i + 1] === 0x4d && head[i + 2] === 0x54 && head[i + 3] === 0x31) return tiffOrientation(b, i + 4);
	}
	return undefined;
}

/**
 * Draw `image` onto a new canvas the right way up for EXIF `orientation` (1–8), scaled by `scale`. A canvas
 * rather than CSS, so what is shown and anything read back from it agree.
 */
export function oriented(image: ImageBitmap, orientation: number, scale = 1): OffscreenCanvas {
	const w = Math.max(1, Math.round(image.width * scale));
	const h = Math.max(1, Math.round(image.height * scale));
	const turned = orientation >= 5 && orientation <= 8;
	const canvas = new OffscreenCanvas(turned ? h : w, turned ? w : h);
	const ctx = canvas.getContext('2d')!;
	const t: Record<number, [number, number, number, number, number, number]> = {
		2: [-1, 0, 0, 1, w, 0],
		3: [-1, 0, 0, -1, w, h],
		4: [1, 0, 0, -1, 0, h],
		5: [0, 1, 1, 0, 0, 0],
		6: [0, 1, -1, 0, h, 0],
		7: [0, -1, -1, 0, h, w],
		8: [0, -1, 1, 0, 0, w]
	};
	if (t[orientation]) ctx.transform(...t[orientation]);
	ctx.imageSmoothingQuality = 'high';
	ctx.drawImage(image, 0, 0, w, h);
	return canvas;
}

export type ShownPreview = {
	/** An object URL of the preview, upright and at most `longEdge` on its long side. Revoke when done. */
	url: string;
	/** The embedded JPEG's own size, as the camera stored it. */
	stored: [number, number];
};

/** The RAW's embedded preview, ready for an `<img>`; null when the file has none this browser can decode. */
export async function showPreview(bytes: Uint8Array<ArrayBuffer>, longEdge: number): Promise<ShownPreview | null> {
	const found = largestEmbeddedJpeg(bytes);
	if (!found) return null;
	let bitmap: ImageBitmap;
	try {
		// The browser applies a JPEG's own EXIF orientation as it decodes; the RAW's applies only where the
		// JPEG states none, which is the usual case for a NEF or CR2 preview.
		bitmap = await createImageBitmap(new Blob([bytes.subarray(found.start, found.end)], { type: 'image/jpeg' }), { imageOrientation: 'from-image' });
	} catch {
		return null;
	}
	const turn = found.orientation === undefined ? (rawOrientation(bytes) ?? 1) : 1;
	const canvas = oriented(bitmap, turn, Math.min(1, longEdge / Math.max(bitmap.width, bitmap.height)));
	bitmap.close();
	const blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.9 });
	return { url: URL.createObjectURL(blob), stored: [found.width, found.height] };
}
