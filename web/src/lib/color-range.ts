// SPDX-License-Identifier: Apache-2.0
/**
 * Colour-range layers in the browser (DESIGN.md §15, "Colour ranges"): each layer's selection made by the same
 * Rust the worker runs (`sidecar_color::color_range::select`, through wasm) from the same input — the proxy at
 * `SELECT_SIZE` and the model mask it is limited to — and kept as a texture until its range or the white balance
 * it is judged under changes.
 */
import { api } from '$lib/api';
import type { ColorPacker } from '$lib/color/pack';
import { fromHalf } from '$lib/color/transfer';
import type { ColorRange } from '$lib/generated/edit/ColorRange';

/** `color_range::SIZE`: the long edge the selection is computed at. */
export const SELECT_SIZE = 1024;

type Pixels = { width: number; height: number; rgba: Float32Array };
type Grey = { width: number; height: number; bytes: Uint8Array };

export class ColorSelections {
	private proxy?: Pixels | 'loading';
	private within = new Map<string, Grey | 'loading' | 'missing'>();
	private made = new Map<number, { key: string; texture: GPUTexture }>();

	constructor(
		private device: GPUDevice,
		private assetKey: string,
		private packer: ColorPacker
	) {}

	/**
	 * Layer `i`'s selection under the develop uniform the edit packs to, or `undefined` while what it needs loads —
	 * `ready` is called when it lands. `judgedUnder` names what of the edit the selection depends on (its white
	 * balance), so a change there makes it again.
	 */
	texture(i: number, range: ColorRange, uniform: Uint8Array, judgedUnder: string, ready: () => void): GPUTexture | undefined {
		const proxy = this.loadProxy(ready);
		const within = range.within ? this.loadWithin(range.within, ready) : null;
		if (!proxy || within === undefined) return undefined;
		const key = JSON.stringify([range, judgedUnder]);
		const had = this.made.get(i);
		if (had?.key === key) return had.texture;
		const sel = this.packer.colorSelect(range, uniform, proxy, within ?? null);
		const texture = had?.texture.width === proxy.width && had.texture.height === proxy.height ? had.texture : this.device.createTexture({ label: `colour range ${i}`, size: [proxy.width, proxy.height], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		if (had && had.texture !== texture) had.texture.destroy();
		this.device.queue.writeTexture({ texture }, sel, { bytesPerRow: proxy.width }, [proxy.width, proxy.height]);
		this.made.set(i, { key, texture });
		return texture;
	}

	private loadProxy(ready: () => void): Pixels | undefined {
		if (this.proxy === 'loading') return undefined;
		if (this.proxy) return this.proxy;
		this.proxy = 'loading';
		api(`/api/assets/${this.assetKey}/proxy?long_edge=${SELECT_SIZE}`)
			.then(async (r) => {
				const [width, height] = [Number(r.headers.get('x-width')), Number(r.headers.get('x-height'))];
				const half = new Uint16Array(await r.arrayBuffer());
				const rgba = new Float32Array(half.length);
				for (let k = 0; k < half.length; k++) rgba[k] = fromHalf(half[k]);
				this.proxy = { width, height, rgba };
				ready();
			})
			.catch(() => (this.proxy = undefined));
		return undefined;
	}

	/** A model mask's bytes, `null` when this server can't make it (nothing is selected then), `undefined` while it loads. */
	private loadWithin(name: string, ready: () => void): Grey | null | undefined {
		const m = this.within.get(name);
		if (m === 'loading') return undefined;
		if (m === 'missing') return null;
		if (m) return m;
		this.within.set(name, 'loading');
		fetch(`/api/assets/${this.assetKey}/masks/${name}`)
			.then(async (r) => {
				if (!r.ok) throw new Error(`${r.status}`);
				const bmp = await createImageBitmap(await r.blob());
				const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d')!;
				c.drawImage(bmp, 0, 0);
				const d = c.getImageData(0, 0, bmp.width, bmp.height).data;
				const bytes = new Uint8Array(bmp.width * bmp.height);
				for (let k = 0; k < bytes.length; k++) bytes[k] = d[k * 4];
				this.within.set(name, { width: bmp.width, height: bmp.height, bytes });
			})
			.catch(() => this.within.set(name, 'missing'))
			.finally(ready);
		return undefined;
	}
}
