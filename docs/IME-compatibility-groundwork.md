## Task: Add keyboard and IME compatibility groundwork

Knightty now has:

- Headless `InputRouter`
- Injectable clipboard / PTY / URL opener / cursor ports
- Hyperlink Ctrl+Click behavior
- Selection and clipboard behavior
- Paste and bracketed paste support
- Scrollback and alternate screen behavior
- Terminal compatibility smoke documentation
- Recorded GUI smoke results

Recent verification passed:

- `cargo fmt --all -- --check`
- `CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target cargo clippy --workspace --all-targets`
- `CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target cargo test --workspace`
- `git diff --check`

`git diff --check` may show CRLF conversion warnings only; those are not whitespace errors.

### Goal

Start keyboard and IME compatibility hardening without large unrelated changes.

Focus on documenting current behavior, adding deterministic routing tests where possible, and making small fixes only for confirmed issues.

### Requirements

#### 1. Audit keyboard input routing

Review keyboard handling in the app/input layer.

Check:

- Printable text input
- Enter
- Backspace
- Tab
- Escape
- Arrow keys
- Home / End
- PageUp / PageDown
- Insert / Delete
- Function keys
- Ctrl combinations
- Alt combinations
- Ctrl+Shift shortcuts
- Shift+Insert paste
- Ctrl+Shift+C copy
- Ctrl+Shift+V paste
- Ctrl+Shift+PageUp / PageDown scroll shortcuts

Ensure terminal input and application shortcuts do not conflict accidentally.

#### 2. Add keyboard routing tests

Using the existing headless `InputRouter` harness, add tests for:

- Printable ASCII text is written to PTY.
- Enter writes the expected newline/control sequence.
- Backspace writes the expected byte.
- Tab writes `\t`.
- Escape writes ESC.
- Arrow keys write expected terminal sequences.
- Ctrl+Shift+C copies and does not send Ctrl+C to PTY.
- Plain Ctrl+C sends interrupt to PTY and is not treated as copy.
- Ctrl+Shift+V pastes clipboard text.
- Shift+Insert pastes clipboard text.
- Ctrl+Shift+PageUp / PageDown route to scrollback and are not written to PTY.
- Shortcut handling does not clear selection except where intended.
- Printable input clears selection before writing to PTY.

Prefer testing behavior through existing router APIs and fake PTY/clipboard ports.

#### 3. Add IME behavior documentation

Create or update:

```text
docs/dev/keyboard-ime-compatibility.md
````

Document:

* Current platform target: Windows first
* Expected behavior for Japanese IME
* What is currently verified
* What is manual pending
* Known limitations if any

Include manual smoke checks for:

* Japanese IME composition
* committed Japanese text reaches PTY
* preedit text does not get prematurely sent as PTY input if applicable
* Enter during composition confirms composition rather than sending shell Enter, if supported by the platform event model
* Escape during composition cancels composition if supported
* switching IME on/off does not break shortcuts
* Ctrl+Shift+C/V still work while IME is available
* ASCII typing after Japanese input still works

#### 4. Investigate winit IME events

Inspect the current winit integration and determine whether Knightty handles IME-related events correctly.

Look for:

* IME preedit event handling
* IME commit event handling
* whether committed IME text enters the same path as normal text
* whether preedit rendering is currently unsupported
* whether unsupported preedit behavior should be documented as a known limitation

Do not implement complex preedit rendering unless it is already straightforward.
If preedit rendering is not supported, document it clearly.

#### 5. Add manual smoke result section

Add a dated section to `docs/dev/keyboard-ime-compatibility.md` for the current smoke attempt.

Use this structure:

```md
## Smoke results - 2026-06-07

### Observed pass

- ...

### Manual pending

- ...

### Known limitations

- ...
```

#### 6. Keep scope controlled

Non-goals:

* Do not rewrite the renderer.
* Do not implement a full IME candidate UI.
* Do not add GUI automation.
* Do not change terminal parser behavior unless a regression is confirmed.
* Do not change documented shortcuts without explicit reason.
* Do not refactor `InputRouter` broadly unless required for testability.

### Verification

Run:

```bat
cargo fmt --all -- --check
set CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
```

### Acceptance criteria

* Keyboard routing tests are added where practical.
* IME behavior is documented.
* Any confirmed small routing bug is fixed with a regression test.
* Existing behavior remains compatible with current terminal smoke docs.
* Verification passes.

## その次

IME / Keyboard が落ち着いたら、次は **設定リファレンスの自動生成化**に戻るのが良いです。

今後 `scrollback_lines`, `hyperlink.allowed_schemes`, `font`, `shell`, `terminal` 系の設定が増えるので、手書き docs を維持するより、Rust 側の config 定義から `default-config.toml` と Astro 用 metadata を生成する方向に寄せるのが安全です。
