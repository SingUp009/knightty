## Task: Run terminal GUI smoke suite and fix only confirmed issues

Knightty now has:

- OSC 8 hyperlink support
- Selection and clipboard integration
- Bracketed paste support
- Scrollback behavior
- Alternate screen behavior
- Headless `InputRouter`
- Injectable side-effect ports for clipboard / PTY / URL opener / cursor
- Automated app/core/render/pty tests
- `docs/dev/terminal-compatibility-smoke.md`
- `docs/dev/windows-test-runner-notes.md`

The latest verification passed with:

- `cargo fmt --all -- --check`
- `CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target cargo clippy --workspace --all-targets`
- `CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target cargo test --workspace`
- `git diff --check`

The normal target directory may fail before cargo starts because of an existing `target\debug\.cargo-lock` access denied issue. Use the temporary target dir workaround when needed.

### Goal

Run the manual GUI compatibility smoke suite and fix only confirmed regressions or behavior mismatches.

Do not add unrelated features.

### Steps

1. Open `docs/dev/terminal-compatibility-smoke.md`.
2. Run the GUI smoke checks on Windows.
3. Record results in a short dev note or checklist section.
4. Fix only issues that are clearly reproduced.
5. Add automated regression tests for any fixed behavior where practical.
6. Re-run verification.

### Areas to check

#### Basic terminal behavior

- Knightty launches.
- Shell prompt appears.
- Typing reaches the shell.
- Resize does not panic.
- Resize does not visibly corrupt the grid.

#### Hyperlinks / OSC 8

- Valid OSC 8 `https://example.com` link renders as link metadata.
- Hover changes cursor.
- Ctrl+Click opens the link.
- Dragging over a link selects text instead of opening.
- `javascript:` does not open.
- Invalid URLs do not open.
- Disallowed URLs are not written to PTY.

#### Selection / Clipboard

- Drag selection works.
- Ctrl+Shift+C copies selected text.
- Ctrl+Shift+C does not clear selection.
- Normal keyboard input clears selection.
- Ctrl+Shift+V pastes clipboard text.
- Paste clears selection.
- Multi-line paste works.
- Japanese/UTF-8 paste works.

#### Bracketed paste

- Paste is raw when bracketed paste mode is disabled.
- Paste is wrapped when bracketed paste mode is enabled.
- Multi-line bracketed paste does not accidentally execute line-by-line in compatible shells/editors.

#### Scrollback

- Large output enters scrollback.
- Mouse wheel scrolls history.
- New output behavior while scrolled up matches the documented policy.
- Scrollback selection copies expected display text.
- OSC 8 metadata in scrollback does not leak URI text into copied selection.

#### Alternate screen

Check with:

- `less`
- `vim` or `nvim`
- `top` or `htop`

Verify:

- Entering alternate screen preserves primary screen.
- Exiting restores primary screen.
- Alternate screen output does not enter primary scrollback.
- Mouse wheel behavior is correct.
- Selection behavior is acceptable.

#### Mouse reporting

- Mouse reporting off: drag selects text.
- Mouse reporting on: application receives mouse events.
- Shift+drag selects text if that is the intended policy.
- Wheel routing matches the intended mode.

#### Unicode / Japanese / wide characters

- Japanese text renders correctly.
- Japanese selection copies correct text.
- Wide characters do not duplicate spacer cells.
- Emoji behavior is documented if imperfect.

#### Window title / focus events

- OSC 0 / OSC 2 title update changes window title.
- Focus events are sent only when enabled.

### Fix policy

For each issue:

- Prefer small targeted fixes.
- Add a regression test if the issue is in core or input routing.
- If it is purely OS/window/render integration, document the manual check.
- Do not refactor large systems unless the issue cannot be fixed locally.

### Verification

Run:

```bat
cargo fmt --all -- --check
set CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
````

### Acceptance criteria

* GUI smoke suite has been run.
* Results are recorded.
* Confirmed issues are either fixed or explicitly documented as known limitations.
* Existing automated tests remain green.
* Any new core/input regression has an automated test.

## その次の本命

GUI smoke が通ったら、次の大きなテーマは **IME / keyboard input hardening** です。

特に日本語環境ではここが重要です。

```text
Phase 6-A: Keyboard and IME compatibility
````

対象:

* 日本語 IME 入力
* preedit / commit handling
* dead keys
* Alt / Ctrl / Shift / Super の取り扱い
* Function keys
* Ctrl+Shift 系 shortcut と shell input の競合
* Windows Terminal / Ghostty / xterm との差分確認

ただし、今すぐ実装に入るより先に、**今回作った smoke suite を一度実走**した方が安全です。
