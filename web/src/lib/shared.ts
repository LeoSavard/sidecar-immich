// SPDX-License-Identifier: Apache-2.0
// A photo shared to Sidecar from the phone, kept on the phone until it is either found or imported.
//
// The share target opens a photo by its checksum, which only finds one the server can already see. A photo
// that isn't in the library yet, or any photo in a deployment with no Immich, matches nothing; without this
// the bytes the share sheet handed over were thrown away at that point. The service worker keeps the file in
// Cache Storage under its checksum, and the pick page offers to import it. Nothing leaves the phone unless
// the person importing it says so.
//
// Used from the service worker and from pages, so it touches neither the DOM nor SvelteKit.

const CACHE = 'sidecar-shared';
/** RAW files run to a hundred megabytes: keep the last few shares, and none for longer than a day. */
const KEEP = 3;
const MAX_AGE_MS = 24 * 3600 * 1000;

const keyFor = (sha1: string) => `/__shared/${sha1}`;

/** A file's SHA-1 as lower-case hex: the asset key the server gives the same bytes. */
export async function sha1Hex(data: ArrayBuffer): Promise<string> {
	const digest = await crypto.subtle.digest('SHA-1', data);
	return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/** Store a shared file under its checksum, and drop shares that are old or beyond the last few. */
export async function keepShared(file: File, sha1: string): Promise<void> {
	const cache = await caches.open(CACHE);
	await cache.put(
		keyFor(sha1),
		new Response(file, {
			headers: {
				'content-type': file.type || 'application/octet-stream',
				'x-file-name': encodeURIComponent(file.name),
				'x-shared-at': String(Date.now())
			}
		})
	);
	const kept: { req: Request; at: number }[] = [];
	for (const req of await cache.keys()) {
		const r = await cache.match(req);
		kept.push({ req, at: Number(r?.headers.get('x-shared-at') ?? 0) });
	}
	kept.sort((a, b) => b.at - a.at);
	const now = Date.now();
	await Promise.all(kept.filter((k, i) => i >= KEEP || now - k.at > MAX_AGE_MS).map((k) => cache.delete(k.req)));
}

/** The file shared under `sha1`, if this phone still holds it. */
export async function sharedFile(sha1: string): Promise<File | null> {
	if (typeof caches === 'undefined') return null;
	const r = await (await caches.open(CACHE)).match(keyFor(sha1));
	if (!r) return null;
	const name = decodeURIComponent(r.headers.get('x-file-name') ?? 'photo');
	return new File([await r.blob()], name, { type: r.headers.get('content-type') ?? '' });
}

/** Let go of a share once it has been imported or found. */
export async function forgetShared(sha1: string): Promise<void> {
	if (typeof caches === 'undefined') return;
	await (await caches.open(CACHE)).delete(keyFor(sha1));
}
