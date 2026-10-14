// SPDX-License-Identifier: Apache-2.0
// Types for what the api sends. Defaults come from Rust via ColorPacker.defaultEdit().

// The edit and its parts are generated from crates/color (`npm run types`, which `npm run generate` runs);
// CI fails if they drift. What's below them is still mirrored by hand.
import type { DisplaySpace } from './generated/edit/DisplaySpace';
import type { EditParams } from './generated/edit/EditParams';
import type { LensProfile } from './generated/edit/LensProfile';
import type { Mixer } from './generated/edit/Mixer';
export type { Crop } from './generated/edit/Crop';
export type { Detail } from './generated/edit/Detail';
export type { DisplaySpace } from './generated/edit/DisplaySpace';
export type { EditParams } from './generated/edit/EditParams';
export type { Frame } from './generated/edit/Frame';
export type { Geometry } from './generated/edit/Geometry';
export type { Lens } from './generated/edit/Lens';
export type { LensProfile } from './generated/edit/LensProfile';
export type { MaskAdjustment } from './generated/edit/MaskAdjustment';
export type { Mixer } from './generated/edit/Mixer';
export type { MixerBand } from './generated/edit/MixerBand';
export type { Stroke } from './generated/edit/Stroke';
export type { Tone } from './generated/edit/Tone';

export type Band = keyof Mixer;

/** Mirrors crates/api Settings. The viewport renders in `outputSpace`, and exports start from it. */
export type Settings = { outputSpace: DisplaySpace; clipHighlights: boolean; clipShadows: boolean };

/** Mirrors `sidecar_raw::lens::LensIdentity`. */
export type LensIdentity = { lensId?: string | null; lensModel?: string | null; lensMake?: string | null; cropFactor?: number | null; focusDistanceM?: number | null };

/** Mirrors `sidecar_xmp::history::Node`, plus the full edit and the exports made from it (`GET …/history`). */
export type HistoryNode = {
	id: string;
	parent?: string;
	etag: string;
	created: string;
	kind: 'original' | 'imported' | 'save' | 'auto';
	label?: string;
	user?: string;
	edit: EditParams;
	exports: { version?: number; file?: string; format?: string; immichAssetId?: string | null }[];
};

export type HistoryState = { head: string; nodes: HistoryNode[] };

export type CameraColor = {
	asShotMul: [number, number, number];
	calibrations: { temperatureK: number; xyzToCamera: number[][] }[];
};

export type Meta = {
	assetKey: string;
	fileName: string;
	processVersion: number;
	info: {
		make: string;
		model: string;
		lens: string;
		width: number;
		height: number;
		sensor: 'bayer' | 'x-trans' | 'other';
		iso: number;
		shutterS: number;
		aperture: number;
		focalLengthMm: number;
		capturedUnix: number;
		color: CameraColor;
		/** Where the camera aimed autofocus, normalized to the oriented image; absent if unknown. */
		afPoint?: { x: number; y: number } | null;
		/** Orientation LibRaw applies (bit set: 4 transpose, 2 flip vertically, 1 flip horizontally). */
		orientationFlip?: number;
		/** The lens correction the file carries itself, or the matched lensfun profile. */
		lensProfile?: LensProfile | null;
		lensIdentity?: LensIdentity | null;
		/**
		 * `developed` means a JPEG, HEIC, PNG or TIFF: already tone-mapped by something else, so it is developed
		 * display-referred (identity at default settings), there is no clipped sensor data to recover, and white
		 * balance is a relative shift, not a measured illuminant. Absent in metadata cached before this existed,
		 * which is always RAW.
		 */
		sourceKind?: SourceKind;
	};
};

export type SourceKind = 'raw' | 'developed';
