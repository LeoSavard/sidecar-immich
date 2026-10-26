// SPDX-License-Identifier: Apache-2.0
// Where the mobile module serves LibRaw for the browser (mobile/libraw/build.sh → mobile/static/libraw/). An absolute
// path, because the main app at / and the module at /m load it alike.
export const LIBRAW_URL = '/m/libraw/sidecar_libraw.js';
