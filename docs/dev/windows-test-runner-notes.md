# Windows Test Runner Notes

## Symptom

Some Windows sandbox launchers can fail before Cargo or Rust tests actually run.
Observed failures include:

```text
CreateProcessAsUserW failed: 5
```

and:

```text
failed to open: target\debug\.cargo-lock
Caused by: Access is denied. (os error 5)
```

## Interpretation

Treat this as a process-launch or filesystem permission failure in the launcher
environment, not as a Rust test failure.

## Workaround

Rerun the same Cargo command outside that launcher. If the workspace `target`
directory is locked or inaccessible, use a temporary target directory:

```cmd
set CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target&& cargo test --workspace
set CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target&& cargo clippy --workspace --all-targets
```

Remove the temporary directory later if you need to reclaim disk space.
