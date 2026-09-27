// SPDX-License-Identifier: Apache-2.0
// How an edit reads in the editor's own terms: slider positions derived from the stored parameters, a
// plain-language list of what differs between two edits, and the sections one edit can take from another.
// The sliders and the history both read through here, so "Whites −12" in the history is the number the
// Whites slider showed.
import type { EditParams, MixerBand } from '$lib/types';
import { clean, decimals, signed } from '$lib/ui/format';

/** Shadows and Highlights move the curve points at 25 % and 75 % — the same curve the curve editor shows. */
export const CURVE_ANCHORS = { shadows: 0.25, highlights: 0.75 } as const;
export type Anchor = keyof typeof CURVE_ANCHORS;

const isAnchor = (x: number) => Object.values(CURVE_ANCHORS).some((a) => Math.abs(x - a) < 1e-6);

export function curveOffset(e: EditParams, which: Anchor): number {
	const x = CURVE_ANCHORS[which];
	const p = e.tone.curve.find((q) => Math.abs(q[0] - x) < 1e-6);
	return p ? clean(((p[1] - x) / 0.15) * 100) : 0;
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

const CONTROLS: Control[] = [
	{ label: 'Exposure', get: (e) => e.exposure.ev, show: (v) => `${signed(v, 2)} EV` },
	{ label: 'Contrast', get: view.contrast, show: (v) => signed(v) },
	{ label: 'Highlights', get: (e) => curveOffset(e, 'highlights'), show: (v) => signed(v) },
	{ label: 'Shadows', get: (e) => curveOffset(e, 'shadows'), show: (v) => signed(v) },
	{ label: 'Whites', get: view.whites, show: (v) => signed(v) },
	{ label: 'Blacks', get: view.blacks, show: (v) => signed(v) },
	{ label: 'Highlight recovery', get: view.highlightRecovery, show: (v) => `${decimals(v)} %` },
	...BANDS.flatMap((b) => FIELDS.map((f): Control => ({ label: `${cap(b)} ${f}`, get: (e) => e.mixer[b][f], show: (v) => signed(v) }))),
	{ label: 'Sharpening', get: (e) => e.detail.sharpenAmount, show: (v) => decimals(v) },
	{ label: 'Sharpening radius', get: (e) => e.detail.sharpenRadius, show: (v) => `${decimals(v, 1)} px` },
	{ label: 'Noise reduction', get: (e) => e.detail.denoiseLuminance, show: (v) => decimals(v) },
	{ label: 'Colour noise reduction', get: (e) => e.detail.denoiseChroma, show: (v) => decimals(v) }
];

/** The curve apart from the two points Highlights and Shadows own, so moving those isn't also "curve edited". */
const ownCurve = (e: EditParams) => JSON.stringify(e.tone.curve.filter((p) => !isAnchor(p[0])));

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
	if (ownCurve(from) !== ownCurve(to)) out.push('Curve edited');
	if (!!from.lens !== !!to.lens) out.push(to.lens ? 'Lens correction on' : 'Lens correction off');
	else if (from.lens && to.lens && JSON.stringify(from.lens) !== JSON.stringify(to.lens)) out.push('Lens correction adjusted');
	return out;
}

/** Sections one version can take from another, as the editor groups its tools. */
export const PICKS: { id: string; label: string; take: (into: EditParams, from: EditParams) => void }[] = [
	{ id: 'lens', label: 'Lens correction', take: (e, f) => (e.lens = structuredClone(f.lens ?? null)) },
	{ id: 'wb', label: 'White balance', take: (e, f) => (e.whiteBalance = structuredClone(f.whiteBalance)) },
	{
		id: 'light',
		label: 'Light',
		take: (e, f) => {
			e.exposure = structuredClone(f.exposure);
			e.tone.contrast = f.tone.contrast;
			e.tone.whiteEv = f.tone.whiteEv;
			e.tone.blackEv = f.tone.blackEv;
			// Highlights and Shadows are curve points: take those two, keep the rest of this curve.
			const anchors = f.tone.curve.filter((p) => isAnchor(p[0]));
			e.tone.curve = [...e.tone.curve.filter((p) => !isAnchor(p[0])), ...structuredClone(anchors)].sort((a, b) => a[0] - b[0]);
		}
	},
	{ id: 'curve', label: 'Curve', take: (e, f) => (e.tone.curve = structuredClone(f.tone.curve)) },
	{ id: 'color', label: 'Color mixer', take: (e, f) => (e.mixer = structuredClone(f.mixer)) },
	{ id: 'detail', label: 'Detail', take: (e, f) => (e.detail = structuredClone(f.detail)) }
];
