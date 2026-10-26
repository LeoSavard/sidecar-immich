// SPDX-License-Identifier: Apache-2.0
// Importing a photo from this device (`POST /api/intake`): from the home page's picker, a drop, or a photo
// shared from the phone that the server couldn't find.
import { api } from '$lib/api';
import { keepOriginal } from '$lib/local/originals';

/** What `GET /api/session` says about this deployment. */
export type Who = { name: string; authRequired: boolean; immich: boolean; intakeFolder: string | null; mobile?: boolean };

export type Destination = 'immich' | 'folder';

/** Where an import can go here: the library when there is one, the editing folder when one is configured. */
export function destinations(who: Who | null): Destination[] {
	return [...(who?.immich ? (['immich'] as const) : []), ...(who?.intakeFolder ? (['folder'] as const) : [])];
}

/**
 * Send the photo to `to` and return the asset key the editor opens it by. On a phone the original is also kept
 * on the phone (`$lib/local/originals`), so the editor can develop it there instead of downloading the server's
 * proxy; a phone short of storage just doesn't keep it, and the import is the same.
 */
export async function importPhoto(file: File, to: Destination): Promise<string> {
	const body = new FormData();
	body.append('file', file, file.name);
	const r = await api(`/api/intake?to=${to}`, { method: 'POST', body });
	const key: string = (await r.json()).assetKey;
	if (matchMedia('(pointer: coarse)').matches) await keepOriginal(file, key).catch((e) => console.warn('not kept on this device:', e));
	return key;
}
