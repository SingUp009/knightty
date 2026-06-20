## Task: Finalize input-router refactor and add terminal compatibility smoke checks

Knightty now has a headless `InputRouter` in `input.rs`.
`main.rs` has been refactored so the winit `ApplicationHandler` is a thin adapter around the router.
Clipboard, PTY writes, URL opening, and cursor updates are injectable ports, and fake harness tests cover hyperlink, selection, clipboard, paste, mouse reporting, and related routing behavior.

Current verification already passed:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets`
- `cargo test --workspace`

Note: On the Windows sandbox runner, `cargo clippy` and `cargo test` initially failed before process startup with `CreateProcessAsUserW failed: 5`. The same commands passed when run outside that launcher. Treat this as a runner/launcher issue, not a test failure.

### Goal

Do not add a large new terminal feature yet.
First, finalize the input-router refactor and add a documented compatibility smoke suite so future changes to scrollback, alternate screen, bracketed paste, mouse reporting, and hyperlinks can be validated consistently.

### Requirements

#### 1. Review the input-router boundary

Inspect the current `input.rs` / `main.rs` split and make small corrections only if necessary.

Ensure:

- `main.rs` remains a thin winit adapter.
- Input behavior lives in `InputRouter`.
- OS side effects are behind injectable ports:
  - clipboard get/set
  - PTY writes
  - URL opening
  - cursor icon updates
- Tests do not open a real browser.
- Tests do not require the real OS clipboard.
- Tests do not require a real GUI window.
- Fake harness types are `#[cfg(test)]` or otherwise not exposed as public production API unless there is a deliberate reason.
- `InputRouter` does not directly call `open::that`, raw clipboard APIs, raw PTY APIs, or winit cursor APIs.

#### 2. Add a compatibility smoke-test document

Create or update a document such as:

```text
docs/dev/terminal-compatibility-smoke.md
````

The document should define a repeatable manual smoke suite for real GUI behavior.

Cover at least:

##### Basic startup

* Launch Knightty.
* Confirm shell prompt appears.
* Confirm typing reaches the shell.
* Confirm resize does not panic or corrupt the visible grid.

##### Hyperlinks / OSC 8

* Show a valid OSC 8 `https://example.com` link.
* Confirm hover changes cursor.
* Confirm Ctrl+Click opens the link.
* Confirm dragging over the link selects text instead of opening.
* Confirm `javascript:` does not open.
* Confirm invalid URLs do not open.

Include Windows PowerShell commands where appropriate.

##### Selection / clipboard

* Drag-select text.
* Ctrl+Shift+C copies selected text.
* Ctrl+Shift+C does not clear selection.
* Normal keyboard input clears selection.
* Ctrl+Shift+V pastes clipboard text.
* Paste clears selection.
* Multi-line paste works.
* Japanese/UTF-8 paste works.

##### Bracketed paste

* Run an application or shell mode that enables bracketed paste.
* Confirm paste is wrapped with bracketed paste sequences when enabled.
* Confirm normal paste is raw when bracketed paste is disabled.

##### Scrollback

* Produce more output than visible rows.
* Mouse wheel scrolls into history.
* New output behavior while scrolled up is documented and verified.
* Selection from scrollback copies expected text.
* `scrollback_lines = 0` disables history if currently supported.

##### Alternate screen

Check at least:

* `less`
* `vim` or `nvim`
* `top` or `htop`

Verify:

* Alternate screen does not pollute primary scrollback.
* Exiting restores the primary screen.
* Mouse wheel behavior is correct inside alternate screen.
* Selection behavior is correct.

##### Mouse reporting

Verify:

* Mouse reporting off: drag selects text.
* Mouse reporting on: mouse events go to the application.
* Shift+drag with mouse reporting on selects text if that is the intended policy.
* Wheel events route correctly depending on mouse reporting mode.

##### Unicode / wide characters

Verify:

* Japanese text renders correctly.
* Wide characters occupy expected cells.
* Selection does not copy wide spacer cells.
* Emoji behavior is documented, even if not perfect yet.

##### Window title / focus events

Verify:

* OSC title update changes the window title.
* Focus events are sent only when focus event mode is enabled.

#### 3. Add scripted terminal fixture tests where practical

Do not try to fully automate the real GUI.

Instead, add non-GUI tests where possible using core/app harnesses.

Add fixture-style tests for escape-sequence-heavy behavior if not already covered:

* OSC 8 hyperlink followed by selection and scrollback
* alternate screen enter/exit with hyperlink metadata
* bracketed paste enable/disable followed by paste
* mouse reporting enable/disable and wheel routing
* title update with OSC 0 / OSC 2
* resize while scrolled
* UTF-8 split across feeds
* CSI split across feeds

Use existing `Terminal` / `InputRouter` APIs rather than adding brittle test-only hooks.

#### 4. Add a short dev note for the Windows launcher issue

Create or update a dev troubleshooting note, for example:

```text
docs/dev/windows-test-runner-notes.md
```

Mention:

* Symptom: `CreateProcessAsUserW failed: 5`
* Context: sandbox launcher can fail before cargo starts
* Interpretation: process-launch failure, not a Rust test failure
* Workaround: rerun the same cargo command outside that launcher

Keep this short. Do not over-engineer it.

### Non-goals

* Do not implement a new renderer.
* Do not rewrite the terminal parser.
* Do not add full GUI automation yet.
* Do not introduce Playwright/Selenium-style GUI testing.
* Do not change public config behavior unless required by a failing test.
* Do not make large architectural changes beyond small cleanup of the router boundary.

### Acceptance criteria

* Existing tests remain green.
* New smoke-test documentation exists and is actionable.
* Any added fixture tests are deterministic and do not require OS clipboard, browser, or GUI.
* `cargo fmt --all -- --check` passes.
* `cargo clippy --workspace --all-targets` passes.
* `cargo test --workspace` passes.

コミットをまだしていないなら、先にこれでもいいです。

```bash
git add .
git commit -m "Refactor input routing into headless testable router"
````
