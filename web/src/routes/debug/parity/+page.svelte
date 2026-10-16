<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { LensPass } from '$lib/gpu/lens';

	type Case = { name: string; width: number; height: number; lens?: boolean; masks?: number; maskSize?: [number, number]; mono?: boolean };
	type Result = { name: string; status: string };

	let results = $state<Result[]>([]);
	let adapterName = $state('');
	let error = $state('');

	const toBase64 = (bytes: Uint8Array) => {
		let s = '';
		for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
		return btoa(s);
	};

	async function run() {
		const out: Record<string, string> = {};
		try {
			const device = await requestDevice();
			const info = device.adapterInfo;
			adapterName = [info?.vendor, info?.architecture, info?.description].filter(Boolean).join(' · ') || 'WebGPU adapter';
			const pass = await DevelopPass.create(device);
			const lens = await LensPass.create(device);
			const cases: Case[] = await (await fetch('/parity/index.json')).json();
			for (const c of cases) {
				const get = async (suffix: string) => (await fetch(`/parity/${c.name}.${suffix}`)).arrayBuffer();
				pass.setSource(c.width, c.height, await get('src.f16'));
				if (c.lens) {
					const target = lens.target(pass.proxy!, 'lens corrected');
					lens.run(pass.proxy!, target, new Uint8Array(await get('lens.bin')));
					pass.setInput(target);
				}
				// Masks as the editor holds them: one byte a pixel, in r8unorm textures.
				const masks: GPUTexture[] = [];
				for (let i = 0; i < (c.masks ?? 0); i++) {
					const [w, h] = c.maskSize!;
					const t = device.createTexture({ size: [w, h], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
					device.queue.writeTexture({ texture: t }, await get(`mask${i}.u8`), { bytesPerRow: w }, [w, h]);
					masks.push(t);
				}
				pass.setMasks(masks.length ? new Uint8Array(await get('masks.bin')) : new Uint8Array(1568), masks, masks.length ? new Float32Array(await get('curves.f32')) : undefined);
				pass.setUniforms(await get('uniform.bin'));
				pass.setMono(c.mono ? new Float32Array(await get('mono.f32')) : new Float32Array(0));
				pass.setToneLut(new Float32Array(await get('lut.f32')));
				pass.render();
				out[c.name] = toBase64(await pass.readback());
				results.push({ name: c.name, status: 'rendered' });
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		(window as unknown as { __parity: unknown }).__parity = { adapter: adapterName, error, images: out };
	}

	$effect(() => {
		run();
	});
</script>

<main class="p-6 font-mono text-sm">
	<h1 class="mb-2 text-lg font-semibold">WebGPU parity</h1>
	<p class="mb-4 text-neutral-500">{adapterName}</p>
	{#if error}<p class="text-red-600">{error}</p>{/if}
	<ul>
		{#each results as r (r.name)}<li>{r.name}: {r.status}</li>{/each}
	</ul>
</main>
