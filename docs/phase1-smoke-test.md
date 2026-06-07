# Phase 1 Smoke Test

Use this checklist before starting Phase 2. The goal is to verify the Phase 1 vertical slice and renderer stabilization only; do not evaluate glyph performance or advanced terminal compatibility here.

## Common Checks

- Run `cargo build --workspace`.
- Run `cargo test --workspace`.
- Run `cargo run -p app` and confirm a window opens.
- Confirm startup logs include the selected `wgpu` adapter name, backend, device type, vendor, device, driver, and driver info.
- Confirm `KNIGHTTY_WGPU_BACKEND=auto cargo run -p app` uses a primary backend such as Vulkan or DX12 when available, or exits with a no-adapter hint when no primary backend is installed.
- Confirm `KNIGHTTY_WGPU_BACKEND=gl cargo run -p app` either starts or exits with a backend/adapter error instead of panicking silently; Mesa llvmpipe/Zink CPU rendering is acceptable only for this explicit fallback check.
- Confirm an invalid override such as `KNIGHTTY_WGPU_BACKEND=bad cargo run -p app` prints the allowed values: `auto`, `vulkan`, `dx12`, `gl`.
- Resize the window and confirm it does not crash.
- Minimize, obscure, or rapidly resize the window and confirm surface timeout/outdated/lost handling does not panic.

## Linux

```bash
cargo run -p app
echo hello
printf '\e[38;2;255;0;0mred\e[0m\n'
ls
pwd
```

Backend override checks:

```bash
KNIGHTTY_WGPU_BACKEND=auto cargo run -p app
KNIGHTTY_WGPU_BACKEND=vulkan cargo run -p app
KNIGHTTY_WGPU_BACKEND=gl cargo run -p app
```

Expected result:

- Shell starts through PTY.
- Typed text reaches the shell.
- `echo hello` output appears in the window.
- `ls` output appears without immediate freezing.
- Enter, Backspace, Tab, Ctrl+C, and arrow keys send the expected basic sequences.
- Window resize updates terminal dimensions and does not crash.

## Windows

```powershell
cargo run -p app
echo hello
dir
cd
```

Backend override checks:

```powershell
$env:KNIGHTTY_WGPU_BACKEND = "auto"; cargo run -p app
$env:KNIGHTTY_WGPU_BACKEND = "dx12"; cargo run -p app
$env:KNIGHTTY_WGPU_BACKEND = "gl"; cargo run -p app
Remove-Item Env:\KNIGHTTY_WGPU_BACKEND
```

Expected result:

- Default shell starts through ConPTY.
- Typed text reaches the shell.
- `echo hello` output appears in the window.
- `dir` output appears without immediate freezing.
- Enter, Backspace, Tab, Ctrl+C, and arrow keys send the expected basic sequences.
- Window resize updates terminal dimensions and does not crash.

For current OSC 8 hyperlink, selection, copy, and paste regression checks on Windows
PowerShell, use the manual checklist in `docs/PHASE4-E.md`.

## Not In Scope

- Glyph atlas optimization.
- Ligatures.
- Kitty keyboard or graphics protocols.
- OSC 8, OSC 133, or OSC 7.
- Scrollback UI.
- Neovim/lazygit full compatibility.
