// SPDX-License-Identifier: Apache-2.0
// The file types Sidecar opens, for file pickers and for deciding which decoder a chosen file needs.
// Mirrors `sidecar_template::{RAW_EXTENSIONS, IMAGE_EXTENSIONS}` by hand; a test in that crate reads this file
// and the manifest's share target and fails when either misses an extension Rust knows.

export const RAW_EXTENSIONS = ['nef', 'nrw', 'arw', 'srf', 'sr2', 'raf', 'cr2', 'cr3', 'crw', 'dng', 'orf', 'rw2', 'pef', 'srw', 'x3f', '3fr', 'iiq', 'erf', 'mef', 'mos', 'rwl'];

export const IMAGE_EXTENSIONS = ['jpg', 'jpeg', 'png', 'tif', 'tiff', 'heic', 'heif'];

const extension = (name: string) => name.slice(name.lastIndexOf('.') + 1).toLowerCase();

export const isRaw = (name: string) => RAW_EXTENSIONS.includes(extension(name));

/** For `<input type=file accept=…>`: every RAW, as extensions, since no RAW has a MIME type browsers agree on. */
export const ACCEPT_RAW = RAW_EXTENSIONS.map((e) => `.${e}`).join(',');
