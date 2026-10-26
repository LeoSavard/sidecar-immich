// SPDX-License-Identifier: Apache-2.0
// A JPEG of the edit, made on the phone: the develop pass renders it in sRGB, the page reads it back and the
// browser encodes it. At the size the photo was decoded for editing; a full-resolution render from the phone is
// stage 3's (docs/MOBILE.md).
import type { DevelopPass } from '$lib/gpu/develop';
import { fromHalf, srgbEncode } from '$lib/color/transfer';

/** Display-linear RGBA16F, as `DevelopPass.readback` returns it → an 8-bit sRGB JPEG. */
export async function encodeJpeg(develop: DevelopPass, quality = 0.92): Promise<Blob> {
	const t = develop.output!;
	const bytes = await develop.readback();
	const half = new Uint16Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 2);
	const rgba = new Uint8ClampedArray(t.width * t.height * 4);
	const encode = (v: number, noise: number) => Math.round(srgbEncode(Math.min(Math.max(v, 0), 1)) * 255 + noise);
	for (let i = 0; i < rgba.length; i += 4) {
		// Triangular dither, as the server's export does, so a smooth sky doesn't band at 8 bits.
		const noise = Math.random() - Math.random();
		rgba[i] = encode(fromHalf(half[i]), noise);
		rgba[i + 1] = encode(fromHalf(half[i + 1]), noise);
		rgba[i + 2] = encode(fromHalf(half[i + 2]), noise);
		rgba[i + 3] = 255;
	}
	const canvas = new OffscreenCanvas(t.width, t.height);
	canvas.getContext('2d')!.putImageData(new ImageData(rgba, t.width, t.height), 0, 0);
	return canvas.convertToBlob({ type: 'image/jpeg', quality });
}

/** Hand the file to the phone: the share sheet where there is one (Save to Photos on an iPhone), a download otherwise. */
export async function saveToDevice(blob: Blob, name: string): Promise<'shared' | 'downloaded' | 'cancelled'> {
	const file = new File([blob], name, { type: blob.type });
	if (navigator.canShare?.({ files: [file] })) {
		try {
			await navigator.share({ files: [file] });
			return 'shared';
		} catch (e) {
			if (e instanceof DOMException && e.name === 'AbortError') return 'cancelled';
		}
	}
	const a = Object.assign(document.createElement('a'), { href: URL.createObjectURL(file), download: name });
	a.click();
	setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
	return 'downloaded';
}
