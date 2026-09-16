// SPDX-License-Identifier: Apache-2.0
import { goto } from '$app/navigation';

export class HttpError extends Error {
	constructor(
		readonly status: number,
		message: string,
		readonly body?: unknown
	) {
		super(message);
	}
}

/** fetch that sends signed-out visitors to /signin and turns error bodies into messages. */
export async function api(path: string, init?: RequestInit): Promise<Response> {
	const r = await fetch(path, init);
	if (r.status === 401) {
		await goto(`/signin?next=${encodeURIComponent(location.pathname + location.search)}`);
		throw new HttpError(401, 'Signed out');
	}
	if (!r.ok && r.status !== 412) {
		const text = await r.text();
		throw new HttpError(r.status, text || r.statusText);
	}
	return r;
}

export const thumbnailUrl = (immichAssetId: string) => `/api/immich/assets/${immichAssetId}/thumbnail`;
