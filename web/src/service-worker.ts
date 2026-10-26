// SPDX-License-Identifier: Apache-2.0
// Android share target: the Immich app shares the original file; hash it here and open Sidecar by checksum.
// The bytes stay on the phone: kept here under the checksum, so that when the server finds nothing to match
// the pick page can offer to import them (`$lib/shared`), and sent only if the person chooses that.
/// <reference lib="webworker" />
import { keepShared } from '$lib/shared';

declare const self: ServiceWorkerGlobalScope;

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));

self.addEventListener('fetch', (event) => {
	const url = new URL(event.request.url);
	if (event.request.method !== 'POST' || url.pathname !== '/share') return;
	event.respondWith(
		(async () => {
			const form = await event.request.formData();
			const file = form.get('media');
			if (!(file instanceof File)) return Response.redirect('/', 303);
			const digest = await crypto.subtle.digest('SHA-1', await file.arrayBuffer());
			const sha1 = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
			// A full or unavailable cache costs only the import offer, never the open itself.
			await keepShared(file, sha1).catch((e) => console.warn('Sidecar: could not keep the shared photo', e));
			const q = new URLSearchParams({ sha1, name: file.name, size: String(file.size) });
			// No capture date here: a shared file's lastModified is the download time, not when it was taken.
			return Response.redirect(`/open?${q}`, 303);
		})()
	);
});
