# Keyboard and IME Compatibility

Knightty currently targets Windows first for keyboard and IME hardening. Other
platforms should use the same routing model where possible, but this document
records the Windows behavior and pending Japanese IME checks first.

## Current routing model

- `InputRouter` owns terminal input routing and is tested without a GUI window.
- Application shortcuts are checked before terminal input:
  - `Ctrl+Shift+C` copies selection and is not sent as `Ctrl+C`.
  - `Ctrl+Shift+V` pastes clipboard text.
  - `Shift+Insert` pastes clipboard text.
  - `Ctrl+Shift+PageUp` and `Ctrl+Shift+PageDown` scroll the viewport and are
    not written to the PTY.
- Plain terminal input writes to the PTY and clears selection first.
- Printable keyboard text and IME committed text enter the same text-input path.
- Unambiguous named keys use baseline terminal sequences for Enter, Backspace,
  Tab, Escape, arrows, Home/End, PageUp/PageDown, Insert/Delete, and F1-F12.
- Plain Ctrl character mappings use C0 control bytes for letters and common
  ASCII control characters.
- Alt plus printable text is sent as ESC-prefixed text.

## Expected Japanese IME behavior

- IME events are enabled for the window with `winit`.
- Committed Japanese text should be written to the PTY as UTF-8.
- Preedit text should not be sent to the PTY before commit.
- Enter during composition should be handled by the platform IME as composition
  confirmation where `winit` provides IME preedit/commit events.
- Escape during composition should be handled by the platform IME as composition
  cancellation where supported by the platform event model.
- Switching IME on and off should not change Knightty shortcuts.
- After Japanese input commits, normal ASCII typing should still write to the
  PTY through the same text path.

## Currently verified

- Headless routing covers printable ASCII text, IME committed text, named key
  terminal sequences, Ctrl mappings, Alt+printable text, copy/paste shortcuts,
  Shift+Insert paste, scroll shortcuts, and selection-clearing behavior.
- `WindowEvent::Ime(Ime::Commit(text))` is routed to the same text input path as
  normal printable text.
- `WindowEvent::Ime(Ime::Preedit(_, _))` is not routed to the PTY.

## Manual pending

- Japanese IME composition in the real Windows GUI.
- Committed Japanese text reaches the shell/PTY.
- Preedit text is not prematurely sent as shell input.
- Enter during composition confirms composition rather than sending shell Enter.
- Escape during composition cancels composition rather than sending terminal ESC.
- IME on/off switching does not break shortcuts.
- `Ctrl+Shift+C` and `Ctrl+Shift+V` still work while IME is available.
- ASCII typing after Japanese input still works.

## Known limitations

- Preedit rendering is not implemented. Knightty currently relies on the
  platform IME UI during composition and only handles committed text.
- IME candidate cursor positioning is not implemented yet because
  `set_ime_cursor_area` is not wired to the terminal cursor cell.
- No in-app IME candidate UI is planned for this groundwork pass.
- Modified-key terminal sequences for Ctrl/Alt plus navigation or function keys
  are intentionally not expanded beyond the baseline mappings documented above.

## Smoke results - 2026-06-07

### Observed pass

- Static audit confirmed the app now enables `winit` IME events for the window.
- Static audit confirmed `Ime::Commit(text)` enters the same router path as
  printable text.
- Static audit confirmed preedit events are ignored rather than written to the
  PTY.
- Headless routing tests cover keyboard and shortcut behavior without requiring
  GUI automation.
- Verification passed with `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets`, `cargo test --workspace`, and
  `git diff --check`.

### Manual pending

- Japanese IME composition and commit behavior in the live Windows GUI.
- Real shell observation that committed Japanese text reaches the PTY.
- Confirmation that preedit text, Enter, and Escape are consumed by the IME
  during composition where supported.
- Confirmation that shortcut behavior remains stable with IME enabled.

### Known limitations

- Preedit text is not rendered by Knightty.
- IME candidate cursor area is not positioned at the terminal cursor.
- Full IME candidate UI is out of scope for this pass.
