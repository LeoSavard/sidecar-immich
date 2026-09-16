// SPDX-License-Identifier: Apache-2.0
// Per-device drafts in IndexedDB: slider state survives a closed tab, a GPU reset or Safari's back swipe.
import type { EditParams } from '$lib/types';

export type Draft = { edit: EditParams; baseEtag: string; savedAt: number };

const DB = 'sidecar';
const STORE = 'drafts';

function db(): Promise<IDBDatabase> {
	return new Promise((resolve, reject) => {
		const req = indexedDB.open(DB, 1);
		req.onupgradeneeded = () => req.result.createObjectStore(STORE);
		req.onsuccess = () => resolve(req.result);
		req.onerror = () => reject(req.error);
	});
}

async function tx<T>(mode: IDBTransactionMode, run: (s: IDBObjectStore) => IDBRequest): Promise<T> {
	const d = await db();
	return new Promise((resolve, reject) => {
		const req = run(d.transaction(STORE, mode).objectStore(STORE));
		req.onsuccess = () => resolve(req.result as T);
		req.onerror = () => reject(req.error);
	});
}

export const loadDraft = (key: string) => tx<Draft | undefined>('readonly', (s) => s.get(key)).catch(() => undefined);
export const saveDraft = (key: string, draft: Draft) => tx('readwrite', (s) => s.put(draft, key)).catch(() => undefined);
export const clearDraft = (key: string) => tx('readwrite', (s) => s.delete(key)).catch(() => undefined);
