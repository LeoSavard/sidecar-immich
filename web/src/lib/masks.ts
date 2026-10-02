// SPDX-License-Identifier: Apache-2.0
// Masks the models made for a photo (the worker's masks.rs): which there are, the faces found, and each mask
// as a texture in the proxy's frame — oriented, before lens correction, like the focus mask. This is what a
// tool calls: `library.texture('skin')`. The first request for a photo has the worker compute them all, which
// takes a few seconds (minutes, once, on a new install while the models tune themselves to the GPU).
import { api } from '$lib/api';

export type Face = {
	score: number;
	/** x, y, width, height, as fractions of the photo. */
	bounds: [number, number, number, number];
	/** The subject's right eye, left eye, nose tip, right and left mouth corners. */
	landmarks: [number, number][];
};

export type MaskSet = { available: boolean; names: string[]; faces: Face[] };

export const MASK_LABELS: Record<string, string> = {
	subject: 'Subject',
	skin: 'Skin',
	'face-skin': 'Face skin',
	'body-skin': 'Body skin',
	hair: 'Hair',
	clothes: 'Clothes',
	brush: 'Brush'
};

/** A layer painted from nothing, which needs no models. */
export const BRUSH = 'brush';

export type BrushMode = 'off' | 'paint' | 'erase';
/** The brush as the panel sets it: size, feather and flow on 0…100 sliders (`brushStroke` turns them into a stroke). */
export type BrushSettings = { mode: BrushMode; size: number; feather: number; flow: number };

/** A stroke's settings from the panel's: size 0…100 is a radius from 0.3 % to 25 % of the long edge, evenly in ratio. */
export function brushStroke(b: BrushSettings) {
	return { erase: b.mode === 'erase', radius: 0.003 * (0.25 / 0.003) ** (b.size / 100), feather: b.feather / 100, flow: b.flow / 100 };
}

const NONE: MaskSet = { available: false, names: [], faces: [] };

/** A greyscale PNG's values, decoded without any colour management touching them. */
export async function decodeGrey(png: ArrayBuffer): Promise<{ width: number; height: number; data: Uint8Array }> {
	const bitmap = await createImageBitmap(new Blob([png], { type: 'image/png' }), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
	const { width, height } = bitmap; // close() zeroes these
	const ctx = new OffscreenCanvas(width, height).getContext('2d')!;
	ctx.drawImage(bitmap, 0, 0);
	const rgba = ctx.getImageData(0, 0, width, height).data;
	bitmap.close();
	const data = new Uint8Array(width * height);
	for (let i = 0; i < data.length; i++) data[i] = rgba[i * 4];
	return { width, height, data };
}

/** A greyscale PNG as an `r8unorm` texture. */
export async function greyTexture(device: GPUDevice, png: ArrayBuffer, label: string): Promise<GPUTexture> {
	const { width, height, data } = await decodeGrey(png);
	const texture = device.createTexture({ label, size: [width, height], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
	device.queue.writeTexture({ texture }, data, { bytesPerRow: width }, [width, height]);
	return texture;
}

export class MaskLibrary {
	private set?: Promise<MaskSet>;
	private loading = new Map<string, Promise<GPUTexture>>();
	private loaded = new Map<string, GPUTexture>();

	constructor(
		private device: GPUDevice,
		readonly key: string
	) {}

	/** Which masks this photo has. `available` is false without the models (the plain image) or a GPU. */
	manifest(): Promise<MaskSet> {
		this.set ??= api(`/api/assets/${this.key}/masks`)
			.then((r) => r.json() as Promise<MaskSet>)
			.catch(() => {
				this.set = undefined;
				return NONE;
			});
		return this.set;
	}

	/** A mask, once loaded; `undefined` until then. `load` fetches it. */
	ready(name: string): GPUTexture | undefined {
		return this.loaded.get(name);
	}

	/** Fetch a mask; resolves when `ready(name)` has it. A failure is forgotten, so the next ask tries again. */
	load(name: string): Promise<GPUTexture> {
		let p = this.loading.get(name);
		if (!p) {
			p = api(`/api/assets/${this.key}/masks/${name}`)
				.then((r) => r.arrayBuffer())
				.then((png) => greyTexture(this.device, png, `mask ${name}`))
				.then((t) => {
					this.loaded.set(name, t);
					return t;
				});
			p.catch(() => this.loading.delete(name));
			this.loading.set(name, p);
		}
		return p;
	}

	destroy() {
		for (const t of this.loaded.values()) t.destroy();
		this.loaded.clear();
		this.loading.clear();
	}
}
