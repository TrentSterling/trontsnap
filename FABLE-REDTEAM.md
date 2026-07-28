# TrontSnap: adversarial review findings, 2026-07-27

A 25-agent review ran over the newest work (region OCR, the rewritten clipboard
writer, the hotkey broker, the v0.18.0 theme picker, and the packaging scripts).
Each finding was then handed to a second agent told to **refute** it. 19 raised,
18 survived, 1 refuted.

**Treat that 18/19 survival rate with suspicion.** I re-checked the two highest
destructive-risk findings by hand and one of them was materially overstated (see
"already fixed" below). Verify before believing, especially on severity.

Findings in `packaging/` and `tronteq` are **already fixed**. Everything below is
in `src/` and is yours.

---

## Already fixed, do not redo

- **TrontEQ** `tray_hidden` latch and `good_size` unit bug: fixed and shipped as
  tronteq v0.12.4.
- **`Revert to portable.cmd` bare `exit /b`** discarded the script's real exit
  code, so a failed revert still printed the success banner. Now propagates
  `%errorlevel%`, and resolves the script path from two locations.
- **`revert-to-portable.ps1` copy verification.** A reviewer claimed it deletes
  the Program Files install *before* verifying the portable copy. That is wrong:
  the order is already correct (source checked at step 1, copy at step 3, delete
  at step 4). The **real, smaller** bug was that step 3 used `Test-Path`, which
  passes on a re-run even when `Copy-Item` failed, so a stale or partial portable
  exe could survive as the only binary. Now hash-verified. Good example of a
  finding that was real but not the shape it was reported as.

---

## HIGH

### 1. Clipboard "newest wins" orders by enqueue time, not trigger time
`src/clipboard.rs:106`

OCR is slow (image conversion plus the OCR engine); a plain capture is fast. Fire
the OCR hotkey and then a normal capture immediately after, and the capture
enqueues first, the OCR text enqueues second and **supersedes it**. You asked for
a screenshot and got text on your clipboard. The reverse also holds if timing
flips, so the outcome is non-deterministic.

Suggested: stamp each request with a monotonic sequence number at the moment the
hotkey fires, carry it through to `enqueue`, and refuse to replace a pending
payload whose sequence is newer than the incoming one.

---

## MEDIUM

### 2. Region-OCR gives no feedback on failure or empty result
`src/region_win32.rs:87`

If OCR is unavailable (no language pack), finds no text, or errors, the user
drags a region and then nothing at all happens. Indistinguishable from the hotkey
not firing. The toast infrastructure already exists; a text-only toast saying
"No text found" or "OCR unavailable" closes this.

### 3. New OCR default bind can silently collide with an existing rebind
`src/keyhook.rs:154`

The OCR default is `Ctrl+Alt+PrtSc`. Anyone who had already rebound Full, Region
or Record to that chord now has two actions on one bind; `RegisterHotKey` fails
for the loser and the failure is swallowed. Detect the collision at registration
and surface it in Settings > Hotkeys rather than silently dropping a bind.

### 4. The retry queue is fire-and-forget, so nothing can report failure
`src/clipboard.rs:170`

`set_all` / `set_file` / `set_text` always return `Ok(())` now that the write is
queued. The 90-second give-up path logs and nothing else. The toast therefore
always claims success, which is exactly the class of lie that made the original
"clipboard was busy" bug so hard to chase. Consider a completion channel so a
give-up can surface a toast.

### 5. HGLOBAL leaks on every `SetClipboardData` failure
`src/clipboard.rs:220`

Ownership of the handle passes to the OS only when `SetClipboardData` **succeeds**.
On failure the caller still owns it and must `GlobalFree` it. All three `Payload`
variants leak on that path. Small per occurrence, unbounded across a long-running
tray app that retries.

### 6. Broker ownership decision is one-shot at startup
`src/app.rs:222`

If the broker is slow to appear (AppInfo is busy, a cold disk), `start_broker`
gives up after its poll window, the UI registers locally, and the broker then
starts and cannot register anything. Now neither process owns some binds. The
periodic watchdog relaunches a dead broker but does not renegotiate ownership.

### 7. The accent picker shows the corrected colour, not the raw pick
`src/app.rs:1052`

The v0.18.0 swatch correctly uses `swatch_seed()`, but the `color_edit_button` in
`appearance_ui` still binds to `theme::t().accent`, which is the
readability-corrected value. So the swatch and the picker sitting next to it can
disagree, and dragging in the picker fights the correction. This is the exact
"intent rule" bug from the previous theme arc, resurfacing in the picker itself.

### 8. The Theme window can be dragged under the caption buttons
`src/app.rs:737`

Default `Order::Middle` puts it below the caption overlay, so it can be dragged
into a position where its own title bar sits under the min/max/close buttons and
becomes ungrabbable.

### 9. Install/Revert `.cmd` skip the Medium-integrity launch when already elevated
`packaging/Install TrontSnap.cmd:14`

Running either from an already-elevated shell takes the `:admin` branch
immediately, which installs but never performs the non-elevated `start`. Since
that Medium launch is precisely what makes Windows grant uiAccess, the broker
then runs without it and bare PrtSc silently stops working over elevated windows.

### 10. `bootstrap.ps1` skips rebuilding when a stale binary exists
`bootstrap.ps1:63`

`if (-not (Test-Path $srcExe)) { cargo build }` means an old `target\release`
binary is installed and signed without a rebuild, regardless of source changes.

---

## LOW

11. **`log_line` is not an atomic write** (`src/clipboard.rs:55`). Concurrent
    callers can interleave and garble the log.
12. **Theme window Esc is a non-consumed key check** (`src/app.rs:726`), so it
    can fire alongside another handler reading the same key.
13. **`Launch TrontSnap.cmd` has no elevation check** and inherits the caller's
    integrity level, same uiAccess consequence as finding 9.
14. **`revert-to-portable.ps1` autostart write has no verification**
    (`packaging/revert-to-portable.ps1:106`).

---

## Refuted

- "`create_owner_window()`'s return value is never checked for `HWND(0)`." The
  verifier could not construct a reachable failure.

---

## Suggested order

Finding 1 is the only one that produces a wrong result the user will actually
notice, and it is a direct consequence of adding OCR to a queue built for images.
Findings 2 and 3 are both "the feature silently does nothing", which is the worst
failure mode for a hotkey. Everything else can wait.
