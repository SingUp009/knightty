# Terminal Compatibility Smoke Test

Use this checklist before changing input routing, scrollback, alternate screen,
mouse reporting, hyperlink, clipboard, paste, or title handling. These checks
exercise real GUI behavior and are intentionally manual.

## Basic Startup

- Run `cargo run -p app`.
- Confirm a shell prompt appears.
- Confirm typing reaches the shell.
- Resize the window and confirm the visible grid is not corrupted and the app
  does not panic.

Windows PowerShell:

```powershell
cargo run -p app
echo hello
Get-Location
```

## Hyperlinks / OSC 8

- Show a valid OSC 8 `https://example.com` link.
- Confirm hover changes the cursor to a pointer.
- Confirm Ctrl+Click opens the link.
- Confirm dragging over the link selects text instead of opening.
- Confirm `javascript:` does not open.
- Confirm invalid URLs do not open.

Windows PowerShell:

```powershell
$e = [char]27
[Console]::Write($e + ']8;;https://example.com' + $e + '\example link' + $e + ']8;;' + $e + '\' + [Environment]::NewLine)
[Console]::Write($e + ']8;;javascript:alert(1)' + $e + '\javascript link' + $e + ']8;;' + $e + '\' + [Environment]::NewLine)
[Console]::Write($e + ']8;;not a url' + $e + '\invalid link' + $e + ']8;;' + $e + '\' + [Environment]::NewLine)
```

Expected:

- Only link text is rendered; OSC bytes are not visible.
- Ctrl+Click opens only the `https://example.com` link.
- Rejected links may log a rejection but must not open a browser.

## Selection / Clipboard

- Drag-select text.
- Ctrl+Shift+C copies selected text.
- Ctrl+Shift+C does not clear selection.
- Normal keyboard input clears selection.
- Ctrl+Shift+V pastes clipboard text.
- Paste clears selection.
- Multi-line paste works.
- Japanese / UTF-8 paste works.

Windows PowerShell setup:

```powershell
Set-Clipboard "first line`nsecond line"
# Paste with Ctrl+Shift+V in Knightty.

Set-Clipboard "日本語 UTF-8 paste"
# Paste with Ctrl+Shift+V in Knightty.
```

## Bracketed Paste

- Run an application or shell mode that enables bracketed paste.
- Confirm paste is wrapped with bracketed paste sequences while enabled.
- Confirm paste is raw when bracketed paste is disabled.

Suggested checks:

```powershell
# In PowerShell, PSReadLine commonly enables bracketed paste for interactive input.
Set-Clipboard "first line`nsecond line"
# Paste with Ctrl+Shift+V and confirm the pasted text is inserted as one paste action.
```

On Unix-like shells, also check an editor or REPL that enables bracketed paste,
such as `vim`, `nvim`, or `python`.

## Scrollback

- Produce more output than visible rows.
- Mouse wheel scrolls into history.
- Document and verify new-output behavior while scrolled up.
- Selection from scrollback copies expected text.
- If currently supported, `scrollback_lines = 0` disables history.

Windows PowerShell:

```powershell
1..200 | ForEach-Object { "line $_" }
```

Expected current behavior:

- New PTY output returns the viewport to the live bottom.
- Mouse wheel scrolls primary-screen history when mouse reporting is off.

## Alternate Screen

Check at least:

- `less`
- `vim` or `nvim`
- `top` or `htop`

Verify:

- Alternate screen does not pollute primary scrollback.
- Exiting restores the primary screen.
- Mouse wheel behavior is correct inside alternate screen.
- Selection behavior is correct.

Windows alternatives when Unix tools are unavailable:

```powershell
# Use any installed full-screen terminal app, such as nvim, vim, less, or an interactive TUI.
nvim --version
```

## Mouse Reporting

- Mouse reporting off: drag selects text.
- Mouse reporting on: mouse events go to the application.
- Shift+drag with mouse reporting on selects text.
- Wheel events route according to mouse reporting mode.

Suggested apps:

- `vim` or `nvim` with mouse support enabled.
- `less -R` if available.
- `top` or `htop` on Unix-like systems.

## Unicode / Wide Characters

- Japanese text renders correctly.
- Wide characters occupy expected cells.
- Selection does not copy wide spacer cells.
- Emoji behavior is documented, even if not perfect yet.

Windows PowerShell:

```powershell
Write-Output "日本語 wide chars"
Write-Output "A界B"
Write-Output "emoji: 😀"
```

Expected:

- `界` occupies two cells.
- Selecting `A界B` copies exactly `A界B`.
- Emoji rendering may depend on font support; record current behavior if it is
  imperfect.

## Window Title / Focus Events

- OSC title update changes the window title.
- Focus events are sent only when focus event mode is enabled.

Windows PowerShell title check:

```powershell
$e = [char]27
[Console]::Write($e + ']0;Knightty smoke title' + [char]7)
[Console]::Write($e + ']2;Knightty smoke title 2' + [char]7)
```

Focus event check:

```powershell
$e = [char]27
[Console]::Write($e + '[?1004h')
# Move focus away from and back to the Knightty window.
[Console]::Write($e + '[?1004l')
```

Expected:

- Title changes after OSC 0 / OSC 2.
- Focus in/out bytes are sent only while `?1004` focus mode is enabled.

## Latest Smoke Run - 2026-06-07

Environment:

- Windows desktop, launched from `C:\Users\Romantic\Documents\knightty`.
- Built with `CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target`.
- `nvim` was available; `vim`, `less`, `top`, and `htop` were not found in `PATH`.
- Codex could visually inspect screenshots for launch, shell input, paste, and Unicode output. Pointer interactions, selection dragging, browser opening, and TUI mouse reporting still need direct manual observation.

Results:

| Area | Status | Notes |
| --- | --- | --- |
| Basic startup | pass | `app.exe` launched and rendered a `cmd.exe` prompt. |
| Shell input | pass | `echo hello` reached the shell and printed `hello`. |
| Resize | not available | Not reliably observable through this Codex desktop automation pass. |
| Hyperlinks / OSC 8 | partial pass | Core/render/input automated tests passed for OSC 8 metadata, hover routing, Ctrl+Click open policy, rejected URL schemes, invalid URLs, and drag-over-link selection policy. Manual hover cursor and browser-open observation remains pending. |
| Selection / clipboard | partial pass | Ctrl+Shift+V paste was observed by screenshot. Drag selection and Ctrl+Shift+C copy need manual observation; headless `InputRouter` tests cover the routing. |
| Bracketed paste | partial pass | Core/input automated tests passed for bracketed paste wrapping policy. Manual compatible-shell/editor observation remains pending. |
| Scrollback | partial pass | Core automated tests passed for scrollback, scrolled selection, and hyperlink display-text-only selection. Manual mouse-wheel observation remains pending. |
| Alternate screen | partial pass | Core automated tests passed for alternate-screen preservation and primary scrollback isolation. `nvim` is available for manual follow-up; `vim`, `less`, `top`, and `htop` were not available. |
| Mouse reporting | partial pass | Core/input automated tests passed for mouse mode enable/disable, wheel routing, and Shift+drag policy. Manual TUI observation remains pending. |
| Unicode / wide characters | pass | Ctrl+Shift+V paste and rendering were observed for Japanese text, `A界B`, and emoji. |
| Window title / focus events | partial pass | Core automated tests passed for OSC 0/2 title and focus-event encoding. Manual GUI title/focus observation remains pending. |

No product regression was confirmed during the Codex-observable GUI smoke pass.
The only confirmed mismatch was checklist wording: hyperlink opening is implemented
as Ctrl+Click, not Ctrl+Left.
