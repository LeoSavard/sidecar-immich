// SPDX-License-Identifier: Apache-2.0
// How an edit reads in the editor's own terms: slider positions derived from the stored parameters, a
// plain-language list of what differs between two edits, and the sections one edit can take from another.
// The sliders and the history both read through here, so "Whites −12" in the history is the number the
// Whites slider showed.
import type { EditParams, Geometry, MaskAdjustment, MixerBand } from '$lib/types';
import { MASK_LABELS } from '$lib/masks';
import { clean, decimals, signed } from '$lib/ui/format';

/**
 * A mask layer's Shadows and Highlights still move its curve's points at 25 % and 75 % (unlike the global
 * curve, PROCESS_VERSION 2 gave that its own `tone.highlights`/`shadows` fields instead — see
 * `crates/color/src/params.rs`'s `Tone` and docs/ROADMAP.md).
 */
export const CURVE_ANCHORS = { shadows: 0.25, highlights: 0.75 } as const;
export type Anchor = keyof typeof CURVE_ANCHORS;

const isAnchor = (x: number) => Object.values(CURVE_ANCHORS).some((a) => Math.abs(x - a) < 1e-6);

/** Highlights or Shadows as a mask layer's curve holds them: its point at 75 % or 25 %, ±100 being ±0.15. */
export function curvePointOffset(curve: [number, number][], which: Anchor): number {
	const x = CURVE_ANCHORS[which];
	const p = curve.find((q) => Math.abs(q[0] - x) < 1e-6);
	return p ? clean(((p[1] - x) / 0.15) * 100) : 0;
}

/** `curve` with Highlights or Shadows set to `v`: the slider moves its curve point, 0 removes it. */
export function withCurvePoint(curve: [number, number][], which: Anchor, v: number): [number, number][] {
	const x = CURVE_ANCHORS[which];
	const rest = curve.filter((q) => Math.abs(q[0] - x) >= 1e-6);
	const ends: [number, number][] = rest.length ? rest : [[0, 0], [1, 1]];
	return v === 0 ? rest : [...ends, [x, Math.min(1, Math.max(0, x + (v / 100) * 0.15))] as [number, number]].sort((a, b) => a[0] - b[0]);
}

/** A curve that does nothing, which a layer stores as no curve at all. */
export const isIdentityCurve = (c: [number, number][] | undefined) => !c || c.length === 0 || (c.length === 2 && c[0][0] === 0 && c[0][1] === 0 && c[1][0] === 1 && c[1][1] === 1);

// ---------- geometry

export const NO_GEOMETRY: Geometry = { rotate: 0, flip: false, angle: 0, vertical: 0, horizontal: 0, aspect: 0, crop: { left: 0, top: 0, right: 1, bottom: 1 }, ratio: 0 };

/** Whether a geometry does nothing (the ratio lock alone is a preference, not a change to the picture). */
export function isNoGeometry(g: Geometry | undefined): boolean {
	if (!g) return true;
	const c = g.crop;
	const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;
	return g.rotate % 4 === 0 && !g.flip && g.angle === 0 && g.vertical === 0 && g.horizontal === 0 && g.aspect === 0 && near(c.left, 0) && near(c.top, 0) && near(c.right, 1) && near(c.bottom, 1);
}

/** Crop coordinates before the turn (0…1 of the crop's frame) → the output, turned and mirrored. */
export function twist(g: Geometry, [p, q]: [number, number]): [number, number] {
	let [s, t]: [number, number] = [[p, q], [1 - q, p], [1 - p, 1 - q], [q, 1 - p]][((g.rotate % 4) + 4) % 4] as [number, number];
	if (g.flip) s = 1 - s;
	return [s, t];
}

/** The output → crop coordinates before the turn; `twist` undone. */
export function untwist(g: Geometry, [s, t]: [number, number]): [number, number] {
	if (g.flip) s = 1 - s;
	return [[s, t], [t, 1 - s], [1 - s, 1 - t], [1 - t, s]][((g.rotate % 4) + 4) % 4] as [number, number];
}

/** A point through a row-major projective map. */
export function project(m: number[][], [x, y]: [number, number]): [number, number] {
	const w = m[2][0] * x + m[2][1] * y + m[2][2];
	return [(m[0][0] * x + m[0][1] * y + m[0][2]) / w, (m[1][0] * x + m[1][1] * y + m[1][2]) / w];
}

/** Slider positions, from the stored parameters. */
export const view = {
	contrast: (e: EditParams) => clean((e.tone.contrast - 1) * 100),
	whites: (e: EditParams) => clean(((4.5 - e.tone.whiteEv) / 2) * 100),
	blacks: (e: EditParams) => clean(((-8 - e.tone.blackEv) / 3) * 100),
	highlightRecovery: (e: EditParams) => clean(e.exposure.highlightBlend * 100)
};

const BANDS = ['red', 'orange', 'yellow', 'green', 'aqua', 'blue', 'purple', 'magenta'] as const;
const FIELDS: (keyof MixerBand)[] = ['hue', 'saturation', 'luminance'];
const cap = (s: string) => s[0].toUpperCase() + s.slice(1);

/** One control that reads as a number; `show` formats it as its slider does. */
type Control = { label: string; get: (e: EditParams) => number; show: (v: number) => string };

/** A grain's size on a 35 mm frame, in µm, from its 0–100 slider: `Grain::microns` in sidecar-color. */
export const grainMicrons = (size: number) => 5 * 8 ** (Math.min(Math.max(size, 0), 100) / 100);

const CONTROLS: Control[] = [
	{ label: 'Exposure', get: (e) => e.exposure.ev, show: (v) => `${signed(v, 2)} EV` },
	{ label: 'Contrast', get: view.contrast, show: (v) => signed(v) },
	{ label: 'Highlights', get: (e) => e.tone.highlights ?? 0, show: (v) => signed(v) },
	{ label: 'Shadows', get: (e) => e.tone.shadows ?? 0, show: (v) => signed(v) },
	{ label: 'Whites', get: view.whites, show: (v) => signed(v) },
	{ label: 'Blacks', get: view.blacks, show: (v) => signed(v) },
	{ label: 'Highlight recovery', get: view.highlightRecovery, show: (v) => `${decimals(v)} %` },
	{ label: 'Saturation', get: (e) => e.saturation ?? 0, show: (v) => signed(v) },
	...BANDS.flatMap((b) => FIELDS.map((f): Control => ({ label: `${cap(b)} ${f}`, get: (e) => e.mixer[b][f], show: (v) => signed(v) }))),
	{ label: 'Grain', get: (e) => e.grain?.amount ?? 0, show: (v) => decimals(v) },
	{ label: 'Grain size', get: (e) => e.grain?.size ?? 50, show: (v) => `${Math.round(grainMicrons(v))} µm` },
	{ label: 'Sharpening', get: (e) => e.detail.sharpenAmount, show: (v) => decimals(v) },
	{ label: 'Sharpening radius', get: (e) => e.detail.sharpenRadius, show: (v) => `${decimals(v, 1)} px` },
	{ label: 'Noise reduction', get: (e) => e.detail.denoiseLuminance, show: (v) => decimals(v) },
	{ label: 'Colour noise reduction', get: (e) => e.detail.denoiseChroma, show: (v) => decimals(v) }
];

/** The global curve is manually-drawn points only — Highlights/Shadows are `tone.highlights`/`shadows`, not
 * points in here, so unlike a mask layer's curve (`layerCurve`) this needs no anchor filtering. */
const ownCurve = (e: EditParams) => JSON.stringify(e.tone.curve);

/** What `to` changes from `from`, in the order the editor shows its controls: "Exposure +0.30 EV". */
export function describeChanges(from: EditParams, to: EditParams): string[] {
	const out: string[] = [];
	const [wa, wb] = [from.whiteBalance, to.whiteBalance];
	if (wa.mode !== wb.mode || (wb.mode === 'custom' && (wa.temperatureK !== wb.temperatureK || wa.tint !== wb.tint))) {
		out.push(wb.mode === 'asShot' ? 'White balance as shot' : `White balance ${decimals(wb.temperatureK)} K ${signed(wb.tint)}`);
	}
	for (const c of CONTROLS) {
		const [a, b] = [c.get(from), c.get(to)];
		if (Math.abs(a - b) > 1e-9) out.push(`${c.label} ${c.show(b)}`);
	}
	if (!!from.monochrome !== !!to.monochrome) out.push(to.monochrome ? 'Black and white' : 'Colour');
	else if (from.monochrome && to.monochrome) {
		if (from.monochrome.film !== to.monochrome.film) out.push(`Film type ${to.monochrome.film}`);
		if (from.monochrome.filter !== to.monochrome.filter) out.push(to.monochrome.filter === 'none' ? 'No filter' : `${to.monochrome.filter[0].toUpperCase()}${to.monochrome.filter.slice(1)} filter`);
	}
	if (!!from.film !== !!to.film) out.push(to.film ? 'Film rendering' : 'Neutral rendering');
	else if (from.film && to.film) {
		if (from.film.amount !== to.film.amount) out.push(`Film amount ${decimals(to.film.amount)} %`);
		if (from.film.interlayer !== to.film.interlayer) out.push(`Interlayer effects ${decimals(to.film.interlayer)}`);
	}
	const [b0, b1] = [from.lensBlur?.amount ?? 0, to.lensBlur?.amount ?? 0];
	if (!b0 !== !b1) out.push(b1 ? `Lens blur ${decimals(b1)}` : 'No lens blur');
	else if (from.lensBlur && to.lensBlur) {
		if (b0 !== b1) out.push(`Lens blur ${decimals(b1)}`);
		if (from.lensBlur.focus !== to.lensBlur.focus) out.push(`Focus distance ${decimals(to.lensBlur.focus)}`);
		if (from.lensBlur.range !== to.lensBlur.range) out.push(`Depth of field ${decimals(to.lensBlur.range)}`);
	}
	if ((from.haze ?? 0) !== (to.haze ?? 0)) out.push(`Haze ${signed(to.haze ?? 0)}`);
	if (ownCurve(from) !== ownCurve(to)) out.push('Curve edited');
	out.push(...describeGeometry(from.geometry ?? NO_GEOMETRY, to.geometry ?? NO_GEOMETRY));
	if (!!from.lens !== !!to.lens) out.push(to.lens ? 'Lens correction on' : 'Lens correction off');
	else if (from.lens && to.lens && JSON.stringify(from.lens) !== JSON.stringify(to.lens)) out.push('Lens correction adjusted');
	out.push(...describeMasks(from.masks ?? [], to.masks ?? []));
	return out;
}

/** "Straightened 2.5°", "Cropped", "Turned right": what a geometry change did, in the crop tool's words. */
function describeGeometry(a: Geometry, b: Geometry): string[] {
	const out: string[] = [];
	if (isNoGeometry(b) && !isNoGeometry(a)) return ['Crop and straighten reset'];
	if (a.rotate % 4 !== b.rotate % 4) out.push('Turned');
	if (a.flip !== b.flip) out.push(b.flip ? 'Mirrored' : 'Mirroring undone');
	if (a.angle !== b.angle) out.push(`Straightened ${signed(b.angle, 1)}°`);
	if (a.vertical !== b.vertical) out.push(`Vertical ${signed(b.vertical)}`);
	if (a.horizontal !== b.horizontal) out.push(`Horizontal ${signed(b.horizontal)}`);
	if (a.aspect !== b.aspect) out.push(`Aspect ${signed(b.aspect)}`);
	const [ca, cb] = [a.crop, b.crop];
	const moved = Math.abs(ca.left - cb.left) + Math.abs(ca.top - cb.top) + Math.abs(ca.right - cb.right) + Math.abs(ca.bottom - cb.bottom) > 1e-6;
	// Straightening resizes the crop by itself; only a crop nobody else explains is "Cropped".
	if (moved && out.length === 0) out.push('Cropped');
	return out;
}

const MASK_CONTROLS: { label: string; get: (m: MaskAdjustment) => number; show: (v: number) => string }[] = [
	{ label: 'amount', get: (m) => m.amount, show: (v) => `${decimals(v)} %` },
	{ label: 'exposure', get: (m) => m.exposure, show: (v) => `${signed(v, 2)} EV` },
	{ label: 'contrast', get: (m) => m.contrast, show: (v) => signed(v) },
	{ label: 'highlights', get: (m) => curvePointOffset(m.curve ?? [], 'highlights'), show: (v) => signed(v) },
	{ label: 'shadows', get: (m) => curvePointOffset(m.curve ?? [], 'shadows'), show: (v) => signed(v) },
	{ label: 'whites', get: (m) => m.whites ?? 0, show: (v) => signed(v) },
	{ label: 'blacks', get: (m) => m.blacks ?? 0, show: (v) => signed(v) },
	{ label: 'saturation', get: (m) => m.saturation, show: (v) => signed(v) },
	{ label: 'temperature', get: (m) => m.temperature, show: (v) => signed(v) },
	{ label: 'tint', get: (m) => m.tint, show: (v) => signed(v) }
];

const layerCurve = (m: MaskAdjustment) => JSON.stringify((m.curve ?? []).filter((p) => !isAnchor(p[0])));
const layerMixer = (m: MaskAdjustment) => JSON.stringify(m.mixer ?? null);

/** "Subject exposure +0.50 EV", "Skin mask added": a mask adjustment is named by what its mask covers. */
function describeMasks(from: MaskAdjustment[], to: MaskAdjustment[]): string[] {
	const out: string[] = [];
	const name = (m: MaskAdjustment) => `${m.invert ? 'not ' : ''}${(MASK_LABELS[m.mask] ?? m.mask).toLowerCase()}`;
	const cap = (s: string) => s[0].toUpperCase() + s.slice(1);
	to.forEach((m, i) => {
		const was = from[i];
		if (!was || was.mask !== m.mask) return out.push(`${cap(name(m))} mask added`);
		if (was.invert !== m.invert) out.push(`${cap(name(m))} mask inverted`);
		for (const c of MASK_CONTROLS) if (Math.abs(c.get(was) - c.get(m)) > 1e-9) out.push(`${cap(name(m))} ${c.label} ${c.show(c.get(m))}`);
		if (layerCurve(was) !== layerCurve(m)) out.push(`${cap(name(m))} curve edited`);
		if (layerMixer(was) !== layerMixer(m)) out.push(`${cap(name(m))} colour mixer`);
		const [sa, sb] = [was.strokes?.length ?? 0, m.strokes?.length ?? 0];
		if (sb > sa) out.push(`${cap(name(m))} mask painted`);
		else if (sb < sa) out.push(sb === 0 ? `${cap(name(m))} brush strokes cleared` : `${cap(name(m))} stroke undone`);
	});
	if (from.length > to.length) out.push(from.length - to.length === 1 ? 'Mask removed' : `${from.length - to.length} masks removed`);
	return out;
}

/** Sections one version can take from another, as the editor groups its tools. */
export const PICKS: { id: string; label: string; take: (into: EditParams, from: EditParams) => void }[] = [
	{ id: 'lens', label: 'Lens correction', take: (e, f) => (e.lens = structuredClone(f.lens ?? null)) },
	{
		id: 'geometry',
		label: 'Crop and straighten',
		take: (e, f) => {
			if (f.geometry) e.geometry = structuredClone(f.geometry);
			else delete e.geometry;
		}
	},
	{ id: 'wb', label: 'White balance', take: (e, f) => (e.whiteBalance = structuredClone(f.whiteBalance)) },
	{
		id: 'light',
		label: 'Light',
		take: (e, f) => {
			e.exposure = structuredClone(f.exposure);
			e.tone.contrast = f.tone.contrast;
			e.tone.whiteEv = f.tone.whiteEv;
			e.tone.blackEv = f.tone.blackEv;
			e.tone.highlights = f.tone.highlights ?? 0;
			e.tone.shadows = f.tone.shadows ?? 0;
		}
	},
	{
		id: 'film',
		label: 'Film',
		take: (e, f) => {
			if (f.film) e.film = structuredClone(f.film);
			else delete e.film;
			if (f.monochrome) e.monochrome = structuredClone(f.monochrome);
			else delete e.monochrome;
			if (f.grain) e.grain = structuredClone(f.grain);
			else delete e.grain;
		}
	},
	{ id: 'curve', label: 'Curve', take: (e, f) => (e.tone.curve = structuredClone(f.tone.curve)) },
	{
		id: 'depth',
		label: 'Depth',
		take: (e, f) => {
			if (f.lensBlur) e.lensBlur = structuredClone(f.lensBlur);
			else delete e.lensBlur;
			if (f.haze) e.haze = f.haze;
			else delete e.haze;
		}
	},
	{
		id: 'color',
		label: 'Color mixer',
		take: (e, f) => {
			e.mixer = structuredClone(f.mixer);
			e.saturation = f.saturation ?? 0;
		}
	},
	{ id: 'detail', label: 'Detail', take: (e, f) => (e.detail = structuredClone(f.detail)) },
	{
		id: 'masks',
		label: 'Masks',
		take: (e, f) => {
			if (f.masks?.length) e.masks = structuredClone(f.masks);
			else delete e.masks;
		}
	}
];
