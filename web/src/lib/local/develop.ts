// SPDX-License-Identifier: Apache-2.0
// A photo's proxy made on this device from its original, instead of downloaded from the server (docs/MOBILE.md):
// a RAW through LibRaw in a worker and the server's own ingest shaders on this GPU (`$lib/gpu/ingest`), a developed
// photo through the browser's decoder. Both give the proxy the server would send — the same size, orientation
// and colour, which mobile/scripts/raw-parity.mjs checks — so the editor runs unchanged on it.
import type { ColorPacker } from '$lib/color/pack';
import { isRaw } from '$lib/formats';
import { IngestPass, type Proxy } from '$lib/gpu/ingest';
import type { CameraColor } from '$lib/types';
import { decodeLocally, type SourceSpace } from './decode';
import { decodeRaw, rawOnDevice, type RawInfo } from './raw';

/** Above this, a phone's GPU memory is the risk: the ingest holds about 60 bytes per sensor pixel today. */
const PHONE_MAX_PIXELS = 26e6;

export type Developed = {
	proxy: Proxy;
	/**
	 * The colour model of the proxy where it is not the server's: a developed photo the browser decoded into the
	 * canvas's space (Display P3 where it can). A RAW's is the server's own, to the last digit.
	 */
	camera?: CameraColor;
	/** What it was, for `?inspect`: the RAW's metadata and the colour model this device's LibRaw read from it
	 *  (mobile/scripts/raw-parity.mjs checks it is the server's), or the developed photo's space. */
	raw?: RawInfo;
	rawColor?: CameraColor;
	space?: SourceSpace;
};

const ingests = new WeakMap<GPUDevice, Promise<IngestPass>>();

/**
 * Develop `file` at `longEdge` on this device, or say in a sentence why this device can't (the editor then asks
 * the server, which can).
 */
export async function developOnDevice(device: GPUDevice, file: File, packer: ColorPacker, longEdge: number): Promise<Developed | string> {
	// A RAW is told by its extension, not by trying: a browser that can decode a RAW's embedded JPEG would hand
	// that back as the photo.
	if (!isRaw(file.name)) {
		const photo = await decodeLocally(file, longEdge);
		if (!photo) return "this browser can't decode the file";
		return { proxy: photo, camera: packer.sourceColor(photo.space), space: photo.space };
	}
	if (!(await rawOnDevice())) return 'this server has no RAW decoder for the browser (the mobile module)';
	const raw = await decodeRaw(await file.arrayBuffer());
	if (raw.error !== undefined) return `the RAW decoder here couldn't read it (${raw.error})`;
	if (!raw.mosaic) return 'it holds an already demosaiced image (an sRAW or a linear DNG)';
	const { mosaic } = raw;
	const limit = device.limits.maxTextureDimension2D;
	if (mosaic.width > limit || mosaic.height > limit) return `at ${mosaic.width} × ${mosaic.height} it is larger than this GPU's textures go`;
	if (matchMedia('(pointer: coarse)').matches && mosaic.width * mosaic.height > PHONE_MAX_PIXELS) {
		return `at ${((mosaic.width * mosaic.height) / 1e6).toFixed(0)} megapixels it is more than a phone's GPU should be asked to hold for now`;
	}
	const rawColor = packer.cameraColor(raw.info.color);
	const [r, , b] = rawColor.asShotMul;
	let ingest = ingests.get(device);
	if (!ingest) ingests.set(device, (ingest = IngestPass.create(device)));
	return { proxy: await (await ingest).proxy(mosaic, [r, 1, b], longEdge), raw: raw.info, rawColor };
}
