// SPDX-License-Identifier: Apache-2.0
// The page itself never zooms on a phone: the editor zooms the photo, and a pinch or a double tap anywhere else used to
// zoom the whole interface instead, leaving it zoomed with no way back but another pinch. Three things cause it, and
// each is stopped where it starts:
// - a pinch: `touch-action: pan-x pan-y` on the page (layout.css) forbids it in browsers that honour touch-action
//   for zoom, and Safari, which zooms through its own gesture events whatever touch-action says, is told no here;
// - a double tap: the same `touch-action`, which leaves panning and nothing else;
// - focusing a text field set smaller than 16 px, which iOS zooms to: `maximum-scale=1` in app.html, which iOS
//   reads as "don't zoom on focus" without taking pinch-to-zoom away from pages that allow it.
// The photo keeps its own pinch and double tap: those are pointer events the editor handles, and none of this
// touches them.

/** Stop Safari's page zoom for the life of the page; returns the undo. */
export function preventPageZoom(): () => void {
	const no = (e: Event) => e.preventDefault();
	// Two fingers moving is a pinch; one is a scroll, which stays the browser's.
	const pinch = (e: TouchEvent) => e.touches.length > 1 && e.preventDefault();
	const events = ['gesturestart', 'gesturechange', 'gestureend'];
	for (const t of events) document.addEventListener(t, no, { passive: false });
	document.addEventListener('touchmove', pinch, { passive: false });
	return () => {
		for (const t of events) document.removeEventListener(t, no);
		document.removeEventListener('touchmove', pinch);
	};
}
