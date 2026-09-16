// SPDX-License-Identifier: Apache-2.0
// Per-user settings, kept on the server so the phone and the desktop agree. Loaded once per page.
import { api } from '$lib/api';
import type { Settings } from '$lib/types';

export const DEFAULTS: Settings = { outputSpace: 'srgb', clipHighlights: false, clipShadows: false };

export const SPACE_LABELS: Record<Settings['outputSpace'], string> = { srgb: 'sRGB', 'display-p3': 'Display P3' };

let cached: Promise<Settings> | null = null;

export function settings(): Promise<Settings> {
	cached ??= api('/api/settings')
		.then((r) => r.json())
		.then((s: Partial<Settings>) => ({ ...DEFAULTS, ...s }))
		.catch(() => ({ ...DEFAULTS }));
	return cached;
}

export async function saveSettings(next: Settings): Promise<Settings> {
	const saved = await api('/api/settings', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(next) }).then((r) => r.json());
	cached = Promise.resolve({ ...DEFAULTS, ...saved });
	return cached;
}
