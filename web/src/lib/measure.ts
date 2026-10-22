// SPDX-License-Identifier: Apache-2.0
// The numbers behind a rendering — how much of it is at white or at black, and what one place in it holds —
// so two renderings of the same photo can be compared by reading them rather than by eye (DESIGN.md §8).

/** How much of the picture reached the display's ceiling or floor, as fractions of the pixels counted. */
export type Clipping = { atWhite: number; atBlack: number; counted: number };

/** One place in the picture, averaged over a small square: display-linear RGB and its greatest channel. */
export type Probe = { rgb: [number, number, number]; max: number };

/** Everything measured about one rendering. `label` names which rendering it is. */
export type Reading = { label: string; clipping: Clipping | null; probe: Probe | null };

/** Middle grey, the value the tone map puts a mid-tone at; stops are quoted from here. */
export const GREY = 0.18;

/** Display-linear → the code the file or the screen carries, 0…255. The transfer is sRGB's in both spaces. */
export function encode255(v: number): number {
	const c = Math.min(1, Math.max(0, v));
	return Math.round(255 * (c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055));
}

/** Rec. 709 luminance. The working and both output spaces share the white point, so one set of weights does. */
export function luminance(rgb: [number, number, number]): number {
	return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
}

/** Stops from middle grey, which is how an exposure reads. Null below the point where the numbers stop meaning. */
export function stopsFromGrey(y: number): number | null {
	return y > 1e-6 ? Math.log2(y / GREY) : null;
}

const pct = (n: number, of: number) => {
	if (!of) return '—';
	const p = (n / of) * 100;
	return `${p === 0 ? '0' : p.toFixed(p < 0.1 ? 2 : 1)} %`;
};

const stops = (y: number) => {
	const s = stopsFromGrey(y);
	return s === null ? 'black' : `${s >= 0 ? '+' : '−'}${Math.abs(s).toFixed(2)}`;
};

/** The rows the readout shows, one value per reading, in the order they are read. */
export function rows(readings: Reading[]): { name: string; values: string[] }[] {
	const out: { name: string; values: string[] }[] = [
		{ name: 'at white', values: readings.map((r) => (r.clipping ? pct(r.clipping.atWhite, r.clipping.counted) : '—')) },
		{ name: 'at black', values: readings.map((r) => (r.clipping ? pct(r.clipping.atBlack, r.clipping.counted) : '—')) }
	];
	if (readings.some((r) => r.probe)) {
		out.push(
			{ name: 'probe, 0…255', values: readings.map((r) => (r.probe ? r.probe.rgb.map(encode255).join(' ') : '—')) },
			{ name: 'probe, linear', values: readings.map((r) => (r.probe ? luminance(r.probe.rgb).toFixed(4) : '—')) },
			{ name: 'stops from grey', values: readings.map((r) => (r.probe ? stops(luminance(r.probe.rgb)) : '—')) },
			{ name: 'highest channel', values: readings.map((r) => (r.probe ? r.probe.max.toFixed(4) : '—')) }
		);
	}
	return out;
}

/**
 * The same table as text, for handing the measurement to someone else — a bug report, a note, a conversation
 * about why two renderings differ. `title` names the photo and the space it was read in.
 */
export function asText(title: string, readings: Reading[]): string {
	const names = ['', ...readings.map((r) => r.label)];
	const table = [names, ...rows(readings).map((r) => [r.name, ...r.values])];
	const width = names.map((_, i) => Math.max(...table.map((row) => (row[i] ?? '').length)));
	const line = (row: string[]) => row.map((c, i) => (c ?? '').padEnd(width[i])).join('  ').trimEnd();
	return [title, ...table.map(line)].join('\n');
}
