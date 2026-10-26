// SPDX-License-Identifier: Apache-2.0
// The original of a photo imported from this device, kept on it so the editor can develop it here instead of
// downloading the server's proxy (docs/MOBILE.md): kept in Cache Storage under the photo's asset key, which is
// the SHA-1 of its bytes, so the editor finds it by the key it opens. A photo shared to Sidecar is kept the same
// way by the share target (`$lib/shared`), and found here too.
//
// Only a few, and only for a while: a RAW runs to a hundred megabytes, and an editor that finds nothing here
// simply asks the server, as it always has.
import { sharedFile } from '$lib/shared';

const CACHE = 'sidecar-originals';
const KEEP = 3;
const MAX_AGE_MS = 7 * 24 * 3600 * 1000;

const keyFor = (assetKey: string) => `/__original/${assetKey}`;

/** Keep `file` as the original of `assetKey`, and let go of originals beyond the last few or older than a week. */
export async function keepOriginal(file: File, assetKey: string): Promise<void> {
	if (typeof caches === 'undefined') return;
	const cache = await caches.open(CACHE);
	await cache.put(
		keyFor(assetKey),
		new Response(file, {
			headers: {
				'content-type': file.type || 'application/octet-stream',
				'x-file-name': encodeURIComponent(file.name),
				'x-kept-at': String(Date.now())
			}
		})
	);
	const kept: { req: Request; at: number }[] = [];
	for (const req of await cache.keys()) {
		const r = await cache.match(req);
		kept.push({ req, at: Number(r?.headers.get('x-kept-at') ?? 0) });
	}
	kept.sort((a, b) => b.at - a.at);
	const now = Date.now();
	await Promise.all(kept.filter((k, i) => i >= KEEP || now - k.at > MAX_AGE_MS).map((k) => cache.delete(k.req)));
}

/** This device's copy of the photo `assetKey`, if it still has one. */
export async function originalFile(assetKey: string): Promise<File | null> {
	if (typeof caches === 'undefined') return null;
	const r = await (await caches.open(CACHE)).match(keyFor(assetKey));
	if (!r) return sharedFile(assetKey);
	const name = decodeURIComponent(r.headers.get('x-file-name') ?? 'photo');
	return new File([await r.blob()], name, { type: r.headers.get('content-type') ?? '' });
}
