// SPDX-License-Identifier: Apache-2.0
// Mirrors crates/color/src/params.rs and crates/worker Meta. Defaults come from Rust via ColorPacker.defaultEdit().

export type DisplaySpace = 'srgb' | 'display-p3';

/** Mirrors crates/api Settings. The viewport renders in `outputSpace`, and exports start from it. */
export type Settings = { outputSpace: DisplaySpace; clipHighlights: boolean; clipShadows: boolean };

export type MixerBand = { hue: number; saturation: number; luminance: number };
export type Band = 'red' | 'orange' | 'yellow' | 'green' | 'aqua' | 'blue' | 'purple' | 'magenta';

export type EditParams = {
	processVersion: number;
	whiteBalance: { mode: 'asShot' | 'custom'; temperatureK: number; tint: number };
	exposure: { ev: number; highlightBlend: number };
	tone: { blackEv: number; whiteEv: number; contrast: number; curve: [number, number][] };
	mixer: Record<Band, MixerBand>;
	detail: {
		sharpenAmount: number;
		sharpenRadius: number;
		sharpenIterations: number;
		denoiseLuminance: number;
		denoiseChroma: number;
	};
	/** Absent or null means off (DESIGN.md §15). */
	lens?: Lens | null;
};

/** Mirrors `sidecar_color::params::Lens`: strengths 0…200, 100 being the profile's full correction. */
export type Lens = { profile: string; distortion: number; lateralCa: number; vignetting: number };

/** Mirrors `sidecar_color::lens::LensProfile`. Curves are `[radius, value]` points; radius 1 = half-diagonal. */
export type LensProfile = {
	id: string;
	label: string;
	center: [number, number];
	distortion?: [number, number][] | null;
	caRed?: [number, number][] | null;
	caBlue?: [number, number][] | null;
	vignetting?: [number, number][] | null;
	required?: boolean;
};

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
