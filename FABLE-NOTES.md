# TrontSnap: design notes for the next features

Third file, after `FABLE-HANDOFF.md` (project state) and `FABLE-DECISIONS.md`
(the first round of answers). This one covers annotation, OCR and gallery
multi-select, plus gaps found by reading the code that nobody had listed.

Written 2026-07-27, after v0.18.0 shipped the fast theme picker.

> **Everything here is malleable.** These are notes and recommendations, not a
> spec to follow literally. Where a better idea shows up mid-build, take it. The
> only hard parts are the four decisions in section 1 and the architectural
> constraint in section 2, and even those are open to a good argument.

---

## 1. Decisions already made

| Question | Answer |
|---|---|
| When does the annotation editor open? | **An Edit button on the toast.** Never automatically after a capture. |
| Does saving annotations overwrite the original? | **Yes, overwrite.** No derivative files to manage. |
| Which OCR feature? | **A region OCR hotkey.** Drag a region, get its text on the clipboard. |
| Gallery multi-select and delete? | **Yes.** |

---

## 2. The constraint that shapes annotation: it must be its own process

**`trontsnap edit <path>`, spawned exactly like `trontsnap toast <path>`.**

Why: captures happen while the main window is hidden to tray, and eframe does not
run `update()` on a tray-hidden window. That is already the documented reason the
toast is a separate process. An editor invoked from the toast has the identical
problem, so it needs the identical answer. Trying to open an editor window from
the main process at capture time will appear to do nothing.

Three things fall out of this, all good:

- The gallery can spawn the editor too, with no extra machinery.
- The toast's Edit button just spawns it and closes.
- Neither of them has to know anything about the other.

**Use egui for the editor, not GDI.** The region picker is raw Win32/GDI because
a fullscreen freeze-frame overlay cannot tolerate a GL context warming up. An
editor window is an ordinary window with no such constraint, and it needs a text
caret, undo, and real widgets. Hand-rolling text editing in GDI is misery for no
benefit.

**Toast gotcha for the Edit button.** The toast is a non-activating, always-on-top
tool window, so egui never receives its pointer events; all of its interaction is
Win32 polling (`left_button_down()` plus `cursor_over_window()` in `toast.rs`).
A normal `ui.button()` will not work there. The Edit button has to be a painted
rect plus a manual hit test in that same polling path, alongside the existing
click-to-open and drag-to-send handling.

---

## 3. Annotation

### Suggested MVP toolset

Ranked by how often people actually reach for them in a bug report:

1. **Arrow** (by a distance the most used)
2. **Box outline**
3. **Redaction** (see below)
4. **Text**
5. **Crop**

Obvious next two if it goes well: **step counter** (numbered circles, great for
tutorials) and **highlight** (translucent fill).

### Redaction is a correctness issue, not a cosmetic one

**A gaussian blur over text can be reversed.** If the point is hiding tokens,
keys and paths, a soft blur is a false sense of safety.

Suggested: offer **solid fill** and **pixelate with a chunky block size**, and
make **solid fill the default**. Blur can exist as a third option for the
non-sensitive "de-emphasise this" case, but it should not be what someone reaches
for by accident when hiding an API key.

### Shape model

Suggested: keep a `Vec<Shape>` in memory, render on top of the image, and bake
only on save/copy. That buys undo/redo, and moving or deleting a shape after
placing it, for very little complexity. Undo is just an index into the list.

Do **not** try to persist the vector layer into the PNG or a sidecar. That is a
file-format rabbit hole and nobody asked for re-editable annotations.

### Save behaviour

Overwrite the original file. The gallery watcher already refreshes a thumbnail on
mtime change (`insert_shot` refreshes `taken` and calls `thumbs.forget`, and the
thumb disk key includes mtime), so an edited shot updates in place with no extra
work.

Copy to clipboard on save is probably wanted too, since the usual flow is
annotate then paste.

---

## 4. OCR

### The decision: a region OCR hotkey

Drag a region, get its **text** on the clipboard instead of an image. This is
what PowerToys Text Extractor does, and it is the version people love. The region
picker already exists and already returns a cropped image, so the work is mostly
a second delivery path beside `capture::deliver`.

"Copy text" on a gallery item comes along nearly free once the OCR function
exists, and is worth adding to the right-click menu.

### Deliberately NOT doing: content-searchable history

OCR every capture and index it, so you can search by what is *in* a screenshot.
It compounds beautifully with gallery search and it is a genuinely great feature,
but it needs a backfill over ~18,800 images and somewhere to store the text.
Deferred. If it ever happens, a JSON sidecar keeps it lean; SQLite would mean a
new dependency.

### Technical notes

- `Windows.Media.Ocr` via the `windows` crate the project already depends on. No
  new dependency, no model download, works offline.
- Needs feature flags roughly along the lines of `Media_Ocr`,
  `Graphics_Imaging`, `Storage_Streams`.
- The fiddly part is interop: an `RgbaImage` has to become a `SoftwareBitmap`,
  usually via a BGRA8 buffer and `SoftwareBitmap::CreateCopyFromBuffer`.
- WinRT needs apartment initialisation on the calling thread.
- **`OcrEngine::TryCreateFromUserProfileLanguages()` can return nothing** when no
  OCR language pack is installed. Handle that with a message, not an unwrap. The
  region picker should not die because OCR is unavailable.
- Worth returning bounding boxes even if unused at first; they are what a future
  "select text in the picker" interaction would need.

---

## 5. Gallery multi-select and delete

### Current state

`gallery.rs` has **no keyboard handling and no selection model at all**. Click
copies, double-click opens, right-click menus. There is no way to select more
than one shot, and no way to delete without the context menu, one at a time.

With ~18,800 shots that means there is effectively no cleanup path.

### Suggested

- A `HashSet<usize>` or `Vec<bool>` selection over the filtered index
- **Ctrl+click** toggles one, **Shift+click** selects a range from the anchor
- **Delete** key removes the selection, **Ctrl+A** selects all in the current filter
- Arrow keys move a focus cursor, **Enter** opens
- Selected cells need an obvious visual (the accent ring already used for hover
  is a reasonable starting point, brighter)
- The context menu should act on the whole selection when there is one

`trash` is already a dependency and deletes go to the Recycle Bin, so bulk delete
is recoverable. Worth a confirmation prompt above some threshold anyway.

---

## 6. Gaps found by reading the code, not previously listed

None of these are decided. Raising them because they are real and nobody had
written them down.

### The save folder is hardcoded

Captures go to `Pictures\TrontSnap`, set in `capture.rs`. There are **22
persisted settings and not one of them is a path**. Fine for personal use, and
the first thing any other user will want to change. A `SavePath` setting plus a
folder picker is small.

Related and also absent: no filename pattern configuration. The current pattern
(`TrontSnap_%Y-%m-%d_%H-%M-%S.png`) is second-granularity, which is also why two
captures inside the same second can collide.

### Storage has no management story

~18,800 shots at roughly 800KB is about 15GB, growing every day, with no bulk
delete (section 5), no cleanup-by-age, no duplicate detection, and no size
readout anywhere in the UI. Multi-select delete is the first step; a "Storage"
line in Settings showing total size would be a cheap, honest addition.

### The `Shot` model is metadata-only

`index.rs` models a shot as `{ path, taken, source }`. Dimensions and file size
are read on demand for the tooltip. That is a good, fast design (a 17k-file scan
takes a second or two), and worth preserving: resist the urge to fatten `Shot`
with anything that requires opening the file during a scan.

### Smaller observations

- No "re-copy the last capture to the clipboard" hotkey.
- Region picker has no fixed-size or fixed-aspect mode.
- Recording has no pause.
- Capture is primary-monitor only.

---

## 7. Docs and positioning notes

Carried over because they came up in the same conversation.

**The best untold story is ShareX migration.** TrontSnap reads an existing ShareX
archive and merges it into one timeline, so someone with a decade of history does
not have to abandon it to switch. This appears nowhere in the README or on the
landing page, and it is the single most persuasive thing about the app for its
most likely audience.

**The demo that sells it is four seconds long:** press PrintScreen, the toast
appears with a real preview, drag the toast straight into Discord. That is the
whole product, and it is a GIF, not a paragraph.

**The README of a screenshot tool contains no screenshots.** Worth fixing before
any marketing push.

**The theme engine is genuinely unusual.** No other screenshot tool ships a
randomiser that guarantees readable text on any accent colour. It reads as a
gimmick in a feature list and lands much better as a short animation of the
randomiser being hammered.

---

## 8. Suggested order

1. Gallery multi-select + Delete (unblocks cleanup, small, immediately felt)
2. Region OCR hotkey
3. Annotation editor as `trontsnap edit <path>`
4. README screenshots + demo GIF

Gallery search was ranked first in `FABLE-DECISIONS.md` and still belongs early;
selection and search touch the same file and might be worth doing together.
