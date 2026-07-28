# TrontSnap: decisions, answered

Companion to `FABLE-HANDOFF.md`. Read that first for project state; this file is
just the open questions and Trent's answers. Where an answer contradicts the
handoff doc, **this file wins**.

Answered 2026-07-27.

---

## Q1. What goes in the fast theme picker?

Quick-only (accent swatch, preset dropdown, Randomize, Reset), or the whole
gradient panel (direction, intensity, frost, pegs, harmony)?

**A: the whole panel.**

So the popup gets everything currently living in Settings > Appearance,
including the gradient v2 block. Extract that block from `settings_tab_ui`
(`src/app.rs`, roughly lines 860-943, plus `gradient_editor_ui`) into a single
function and call it from both places. One copy of the controls, two entry
points.

---

## Q2. Does Settings keep its Appearance section?

Keep both entry points, or delete Appearance from Settings so the picker is the
only route?

**A: keep both.**

Same function, called twice. Settings stays complete for anyone who goes looking
there, and the swatch is the fast path.

---

## Q3. Add a Light / Dark / Auto toggle?

An earlier draft asked for one. TrontSnap has no such concept: themes carry their
own polarity (Peach Fuzz resolves light, Hades Fire resolves dark), decided by
the palette's own lightness. There is no global flip like TrontEQ has.

**A: no. Leave it as is, not really needed.**

Do not build a light/dark switch. If light is wanted, pick a light preset.

---

## Q4. What gets built after the picker?

Options offered: gallery search, pin-to-screen, active-window capture, README
screenshots.

**A: gallery search, then README screenshots. The rest is unranked.**

Revised order:

1. **Fast theme picker** (Q1/Q2 above)
2. **Gallery search** — ~18,800 shots and no search box. Filename and date
   filtering; the filter chips (All / New / ShareX) already exist to sit beside.
3. **README screenshots** — it is a screenshot tool whose README has no
   screenshots. Also wants a demo GIF for `docs/index.html`.
4. Pin-to-screen and active-window capture are **parked**, not rejected. Trent
   was lukewarm ("idk the other stuff"). Do not spend time on them unasked.

---

## Q5. Is the "no cloud" positioning permanent?

The About tab says "Fast screenshots, full history, no cloud." Uploads are
ShareX's entire identity.

**A: no cloud for now. Possibly simple uploads later (Imgur-style) if users ask
for them. Definitely NOT self-hosted storage.**

Trent's exact framing, worth keeping intact:

> "I won't be running my own cloud storage that's for sure, and frankly left
> cloud and AI stuff out of it for now to keep app lean."

### This is a product principle, not just an answer to one question

**Lean is the design goal.** Do not propose features that add heavy
dependencies, background services, model downloads, accounts, or network calls.
That rules out, unprompted:

- Self-hosted upload targets, including TrontCloud. The earlier handoff suggested
  this as a differentiator; it is **withdrawn**.
- Semantic or CLIP-style image search. Out of scope.
- Anything that ships or downloads a model.

If uploads ever happen, the shape is a small optional destination (something like
Imgur) that copies a URL to the clipboard, driven by user demand, not built
speculatively.

**Note on OCR**, since the handoff doc listed it as a Tier 2 gap: Windows ships
an OCR engine (`Windows.Media.Ocr`) reachable through the `windows` crate the
project already depends on. No cloud, no model download, no new dependency. That
arguably keeps it inside the lean rule, and it would make "copy the text out of
this screenshot" work offline. But it was not asked for and it is not ranked.
**Raise it as a question before building it; do not assume it is wanted.**

---

## Still unranked from the handoff doc

Not rejected, just not chosen. Do not start these without asking:

- **Annotation / markup** (arrows, boxes, text, blur for redaction). Still the
  single biggest gap versus ShareX, and the reason people stay there. Redaction
  matters specifically because screenshots often contain tokens and paths.
- Multi-monitor capture (currently primary-monitor only).
- Screen colour picker / eyedropper.
- OCR, per the note above.
