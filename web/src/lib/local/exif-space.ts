// SPDX-License-Identifier: Apache-2.0
// Whether a JPEG is a camera's Adobe RGB file that says so only in its EXIF, as the server decides it
// (`SourceSpace::from_tags` in crates/color/src/camera.rs, the review's D11). A camera set to Adobe RGB writes no
// ICC profile; it marks the file with EXIF `ColorSpace` 0xFFFF and `InteropIndex` "R03" (the DCF option-file
// rule), or on some bodies `ColorSpace` 2. A browser knows neither and decodes the file as sRGB, so the page has
// to ask. A file with an ICC profile is left to the browser, which converts it properly.
//
// Written from the JPEG (ITU T.81), TIFF 6.0 and Exif 2.3 specifications. No imports, so
// `node --experimental-strip-types` can load it.

/** True for a JPEG with no ICC profile whose EXIF marks it Adobe RGB. */
export function isExifAdobeRgb(b: Uint8Array): boolean {
	if (b[0] !== 0xff || b[1] !== 0xd8) return false;
	let exif: { tiff: number; end: number } | undefined;
	let p = 2;
	while (p + 4 <= b.length && b[p] === 0xff) {
		const marker = b[p + 1];
		if (marker === 0xd8 || (marker >= 0xd0 && marker <= 0xd7) || marker === 0x01 || marker === 0xff) {
			p += marker === 0xff ? 1 : 2;
			continue;
		}
		if (marker === 0xda || marker === 0xd9) break; // the picture itself starts: no more metadata
		const length = (b[p + 2] << 8) | b[p + 3];
		const body = p + 4;
		if (marker === 0xe2 && ascii(b, body, 12) === 'ICC_PROFILE\0') return false;
		if (marker === 0xe1 && !exif && ascii(b, body, 6) === 'Exif\0\0') exif = { tiff: body + 6, end: p + 2 + length };
		p += 2 + length;
	}
	if (!exif) return false;
	const tags = exifTags(b, exif.tiff, Math.min(exif.end, b.length));
	return tags.colorSpace === 2 || (tags.colorSpace === 0xffff && tags.interop === 'R03');
}

function ascii(b: Uint8Array, at: number, n: number): string {
	let s = '';
	for (let i = 0; i < n && at + i < b.length; i++) s += String.fromCharCode(b[at + i]);
	return s;
}

/** `ColorSpace` (0xA001, in the Exif IFD) and `InteropIndex` (0x0001, in the Interoperability IFD). */
function exifTags(b: Uint8Array, tiff: number, limit: number): { colorSpace?: number; interop?: string } {
	if (tiff + 8 > limit) return {};
	const le = b[tiff] === 0x49 && b[tiff + 1] === 0x49;
	if (!le && !(b[tiff] === 0x4d && b[tiff + 1] === 0x4d)) return {};
	const u16 = (i: number) => (le ? b[i] | (b[i + 1] << 8) : (b[i] << 8) | b[i + 1]);
	const u32 = (i: number) => (le ? (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16)) + b[i + 3] * 2 ** 24 : b[i] * 2 ** 24 + ((b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]));
	/** Each entry's position in the IFD at `offset` (from the TIFF header), by tag. */
	const ifd = (offset: number) => {
		const at = new Map<number, number>();
		const start = tiff + offset;
		if (offset === 0 || start + 2 > limit) return at;
		const count = Math.min(u16(start), 1000);
		for (let i = 0; i < count; i++) {
			const e = start + 2 + 12 * i;
			if (e + 12 > limit) break;
			at.set(u16(e), e);
		}
		return at;
	};
	const ifd0 = ifd(u32(tiff + 4));
	const exifAt = ifd0.get(0x8769);
	if (exifAt === undefined) return {};
	const exif = ifd(u32(exifAt + 8));
	const out: { colorSpace?: number; interop?: string } = {};
	const cs = exif.get(0xa001);
	if (cs !== undefined) out.colorSpace = u16(cs + 8);
	const interopAt = exif.get(0xa005);
	if (interopAt !== undefined) {
		const index = ifd(u32(interopAt + 8)).get(0x0001);
		// "R03\0": four ASCII bytes, which fit in the entry itself.
		if (index !== undefined && u32(index + 4) <= 4) out.interop = ascii(b, index + 8, 3);
	}
	return out;
}
