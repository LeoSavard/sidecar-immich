<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { LensPass } from '$lib/gpu/lens';

	type Case = { name: string; width: number; height: number; lens?: boolean };
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
				pass.setUniforms(await get('uniform.bin'));
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
