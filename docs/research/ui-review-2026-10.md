# A look at the interface — October 2026

Read on 2026-10-26 from the editor, the home page and `/m`, at an iPhone's size (390 × 844, touch) and a desktop's
(1440 × 900), in headless Chromium against a real `sidecar dev`, and from the markup. The photo itself was not on
screen (headless Chromium can't present a WebGPU canvas), so nothing here is about how a picture looks; everything
is about getting to the controls and understanding what they do. Each item says what was seen, why it costs the
person something, and what would fix it. They are ordered by how often someone runs into them.

## Done with this review

- **The page no longer zooms itself on a phone.** A pinch on the panels, a double tap, or focusing a small text
  field zoomed the whole interface, with no way back but another pinch. `touch-action: pan-x pan-y` on the page,
  Safari's own gesture events cancelled, and `maximum-scale=1` against iOS's zoom-on-focus (`$lib/page-zoom.ts`).
  The photo keeps its own pinch and double tap.
- **The camera's own rendering follows the edit.** Beside the edit, it now sits at the edit's zoom and position —
  crop and straighten included — and either side pans and zooms both, with the same pinch, drag, wheel and double
  tap. A tap on it moves the loupe or picks white as a tap on the edit would, at the same point.
- **The loupe goes to 150 % and 200 %.** A double tap (or double click) in the loupe steps 100 → 150 → 200 %, and
  three buttons in its corner pick one; the choice is remembered. At 150 % and 200 % the server renders a smaller
  full-resolution tile and it is shown pixel for pixel, so what is judged is still the real render.

## The owner's answers, and what was built (2026-10-26)

| # | Answer | Built |
|---|---|---|
| 3 | No button: holding is the common way. A quiet hint instead | Shown once per device, after the first change, for five seconds ("Press and hold the photo…", with `\` on a desktop); never again once seen or once compared. The desktop canvas's tooltip says it too |
| 4 | Canadian spelling; a section mustn't share its group's name | "Color" → **Mixer**; no other American spelling was on screen |
| 5 | Hidden, not greyed | `GET /api/capabilities` (the worker's `models`, `gpu`); Depth leaves the tabs and the sidebar without the models, Detail without a GPU, and Masks offers the brush without a sentence about the models |
| 6 | Agreed | The Histogram toggle is with Focus, Camera and sRGB over the photo |
| 9 | Agreed, but only after a press and hold | A handle on the panel's top edge: held for a third of a second it lights up (and buzzes where a phone can), and only then does a drag set the height, which is remembered. A swipe that starts there changes nothing |
| 10 | Agreed; the iPhone's *Edit in Sidecar* shortcut still goes straight to the photo | The home page is the way in on a phone too: it imports into the editing folder by default there, shows a RAW's own preview while uploading, and has lost its "On this phone" link. `/m` now only sends the installed app to it, and the module's manifest covers the whole app (`scope: "/"`), so opening the editor from it stays inside the app |
| 12–14 | Agreed | One level of folding, each section's state remembered; the sRGB / P3 switch is labelled **Output** (it decides the export as well as the screen, so "Display" would mislead); `?` lists the shortcuts, a link at the foot of the sidebar opens the list, and Undo, Redo and Export carry theirs in their titles |

Items 1, 2, 7, 8, 11 and 15–17 had no answer and are not built.

## Worth doing next

### On every photo

1. **The "hide the tools" button wears an ⓘ.** Top left, beside Back, an information icon hides the panel. People
   read ⓘ as "details about this photo" and won't guess it gives the photo the whole screen, or press it expecting
   EXIF and lose their panel. *Fix:* a panel or expand glyph, with the H shortcut in its title as now.
2. **No zoom readout and no "fit".** The only way back from a pinch is another pinch or a double tap that has to
   land on the photo, and nothing says how far in one is. With the loupe now reaching 200 %, the two zooms are
   easy to confuse. *Fix:* a small chip in a corner of the photo — "Fit" or "250 %" — that toggles fit ↔ 100 % on
   tap, shown only when zoomed.
3. **Before/after is invisible.** It exists three ways (press and hold, the `\` key, History's compare), none of them
   shown. The most-used check in any editor shouldn't need to be known about. *Fix:* a before/after button beside
   Camera and Focus, held or toggled, labelled with what it compares against ("As shot", or the version chosen in
   History).
4. **"Colour" holds "Color".** The group is spelled one way and its mixer section the other (`groups` and `sections`
   in the editor). It reads as two different things. *Fix:* one spelling, and a name that says what the section is
   ("Colour mixer", or "Mixer"), since the group and the section otherwise share a name.
5. **Tools that can't work are still offered.** Depth on an image without the models opens to a sentence saying
   it needs the `-ml` image. *Fix:* leave it out of the tabs (or show it dimmed with "needs the -ml image" as its
   only content) where `GET /api/session` says the models aren't there; the same way a JPEG already doesn't show
   what only a RAW has.

### On a phone

6. **The histogram toggle is off the edge.** On a 390-pixel-wide phone the five group tabs fill the row and push the
   histogram button out of sight; it can be reached by scrolling a row nobody knows scrolls. *Fix:* move it up to the
   column of photo toggles (Focus, Camera, sRGB), which is where the histogram appears anyway.
7. **The file name and camera are truncated to nothing** ("iss042e297…", "Nikon D4 · ISO…") to make room for four
   buttons, one of which, **"Saved", is a status dressed as a disabled button.** *Fix:* show saving state as a small
   dot or word by the title, which gives the title the room; undo/redo can be icons without boxes.
8. **The loupe is three taps deep** (Processing → Detail) although it is the way to check focus and detail at any
   time, not only while sharpening. *Fix:* a Loupe toggle with Focus and Camera on the photo, opening it over a
   corner of the photo; Detail keeps it too.
9. **The panel's height is fixed at about 45 % of the screen**, with only four sliders in view. *Fix:* a sheet that
   can be dragged between a peek (the tabs and one row), half and nearly full, remembering where it was left; the
   hide-tools button stays the way to full screen.
10. **Two ways in that now do the same thing.** Since the phone runs the one editor, "Choose a photo on this device"
    on the home page and `/m` both import and open the editor; the home page's "On this phone" link now leads to a
    page that does what the one it is on does. *Fix:* on a phone, let the home page be the way in (it already has
    the folder and the recents) and keep `/m` only as the installed app's start page and share target, without the
    link.
11. **The "Developed on this device" note comes back on every photo.** Once read, it is noise. *Fix:* remember the
    dismissal (per device), and keep the information in the photo's details where it can still be found.

### On a desktop

12. **Two levels of folding.** The sidebar folds by group (COLOUR, LIGHT…) and again by section (White balance,
    Film, Color), with only Colour and Light open at first and nothing remembered. A slider is two clicks from
    sight, and the panel is long when everything is open. *Fix:* one level — group headings as plain headings, the
    sections foldable — and remember what each person leaves open.
13. **sRGB / P3 sits among the view toggles.** It is the display's colour space, a setting chosen once per screen,
    beside Focus peaking and Camera, which are things one flips while working. *Fix:* label it "Display: sRGB / P3"
    or move it to Settings, where the clipping preferences already are.
14. **Keyboard shortcuts are undiscoverable.** H, E, Z, R, J, `\` and ⌘Z all exist; a few are in tooltips. *Fix:* a
    `?` overlay listing them, and every shortcut in its control's title.

### Smaller things

15. The White balance header's eyedropper is a pencil glyph; an eyedropper reads as "pick a neutral" without
    explanation.
16. "Reset to as shot" is grey whether or not there is anything to reset; when it does something, give it the
    normal text colour.
17. Slider values can't be typed on a phone without zooming (now fixed by the page-zoom change), but the numeric field
    is still small at 12 px; on touch screens it could take a tap on the value to open a larger entry.

## How to check what was done

`mobile/scripts/mobile-smoke.mjs` covers the phone's way into the editor. The three changes above were checked in
Chromium against a real server: the page reports `touch-action: pan-x pan-y` and the viewport carries
`maximum-scale=1`; the camera's JPEG lands on the edit's image rectangle at fit (540 × 360 at y = 270 in a 540 × 900
pane), follows a 3.3× zoom of the edit, moves exactly as far as a drag on its own pane (120, 60 px) and zooms both
from its own wheel; the loupe asks for a 280-pixel tile at 100 % and a 140-pixel one at 200 %, and a double click or
a double tap steps it to 150 %. None of it has been tried on a real iPhone yet, which is where the page-zoom change
matters most.
