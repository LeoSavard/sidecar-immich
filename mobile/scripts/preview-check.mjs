// SPDX-License-Identifier: Apache-2.0
// Checks src/local/preview.ts against ExifTool, which reads each RAW format with its own maker-specific parser:
// for every file, the JPEG the browser would show must be byte for byte the largest one ExifTool extracts, and
// the orientation must be the file's.
//
// Usage: node --experimental-strip-types scripts/preview-check.mjs <RAW file or folder>…
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { largestEmbeddedJpeg, rawOrientation } from '../src/local/preview.ts';

const TAGS = ['JpgFromRaw', 'PreviewImage', 'OtherImage', 'ThumbnailImage'];
const files = process.argv.slice(2).flatMap((p) => (statSync(p).isDirectory() ? readdirSync(p).sort().map((f) => join(p, f)) : [p]));
let failed = 0;

const exif = (args, input) => execFileSync('exiftool', args, { input, maxBuffer: 1 << 30 });

for (const file of files) {
	const bytes = readFileSync(file);
	const started = performance.now();
	const found = largestEmbeddedJpeg(new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength));
	const ms = performance.now() - started;
	const ours = found ? bytes.subarray(found.start, found.end) : null;

	// ExifTool's candidates, measured by ExifTool itself.
	const theirs = [];
	for (const tag of TAGS) {
		const jpeg = exif(['-b', `-${tag}`, file]);
		if (jpeg.length < 4) continue;
		const [w, h] = exif(['-s3', '-ImageWidth', '-ImageHeight', '-'], jpeg).toString().trim().split(/\s+/).map(Number);
		theirs.push({ tag, jpeg, pixels: w * h, size: `${w}×${h}` });
	}
	const largest = theirs.filter((t) => t.pixels >= 160 * 160).sort((a, b) => b.pixels - a.pixels)[0];
	const orientation = Number(exif(['-s3', '-n', '-Orientation', file]).toString().trim()) || undefined;

	const name = file.split('/').pop();
	const what = found ? `${found.width}×${found.height}, ${(ours.length / 1e3).toFixed(0)} kB, ${ms.toFixed(0)} ms` : 'none';
	let ok;
	let why;
	if (!largest) {
		ok = !found;
		why = found ? 'ExifTool finds no preview' : 'no preview, as ExifTool says';
	} else {
		ok = !!ours && ours.equals(largest.jpeg);
		why = ok ? `= ExifTool's ${largest.tag} ${largest.size}` : `ExifTool's largest is ${largest.tag} ${largest.size}`;
	}
	const ourOrientation = found?.orientation ?? rawOrientation(new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength));
	// A file ExifTool reads no orientation from may still be upright: only a stated one has to agree, and stating
	// none is upright, which is how the page shows it.
	const oriented = orientation === undefined || (ourOrientation ?? 1) === orientation;
	if (!ok || !oriented) failed++;
	console.log(`${ok && oriented ? 'ok  ' : 'FAIL'} ${name}: ${what} ${why}; orientation ${ourOrientation ?? '-'} (ExifTool ${orientation ?? '-'})`);
}
process.exit(failed ? 1 : 0);
