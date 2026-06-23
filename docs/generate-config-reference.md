## Task: Generate config reference artifacts from Rust config definitions

Knightty already has config loading and validation tests.
A human-written settings reference exists, but it can drift from the Rust implementation.

### Goal

Make the Rust config definition the source of truth for:

- default config output
- config reference metadata
- documentation examples
- drift checks in tests/CI

Do not redesign the config system broadly. Add generation in small steps.

### Requirements

#### 1. Inspect current config model

Review the current config structs and validation in:

- `crates/app/src/config.rs`
- existing config tests
- docs/config reference files, if present

Identify current setting groups, likely including:

- font
- shell
- terminal
- hyperlink
- scrollback
- keyboard/input, if present

#### 2. Add config metadata

Add a maintainable way to describe each setting.

For each setting, capture:

- key path, for example `terminal.scrollback_lines`
- type
- default value
- valid range or accepted values
- short description
- example TOML snippet where useful
- whether restart is required, if known

Prefer keeping metadata near the Rust config definition.
Avoid duplicating defaults manually in multiple places.

Acceptable approaches:

- explicit metadata table in Rust
- trait-based metadata
- macro-based declaration if it stays simple

Do not introduce a large custom schema framework.

#### 3. Generate default config

Add a command or internal function that can emit a complete default config TOML.

Possible command shape:

```bash
knightty --print-default-config
````

or, if startup actions already exist:

```bash
knightty +print-default-config
```

Use the style already present in the project.

The generated config should:

* include all supported settings
* use current default values
* be valid TOML
* be parseable by Knightty
* include comments if straightforward

#### 4. Generate docs metadata

Add a generated artifact suitable for the Astro settings reference.

Preferred output:

```text
docs/generated/config-reference.json
```

or similar.

Each entry should include:

```json
{
  "path": "terminal.scrollback_lines",
  "type": "integer",
  "default": 10000,
  "description": "...",
  "example": "terminal.scrollback_lines = 10000",
  "range": "0..=200000"
}
```

Keep the JSON stable and deterministic:

* sorted by key path
* no nondeterministic formatting
* no machine-local paths

#### 5. Add drift tests

Add tests that ensure:

* generated default config parses successfully
* generated docs metadata includes all known settings
* documented defaults match actual defaults
* range information matches validation where practical
* generated output is stable

If checked-in generated files are used, add a test that fails when generated output differs from committed output.

#### 6. Update docs

Add a short dev note explaining:

* Rust config is the source of truth
* how to regenerate default config
* how to regenerate docs metadata
* how Astro should consume the generated JSON
* how to handle new settings

Suggested file:

```text
docs/dev/config-reference-generation.md
```

#### 7. Scope control

Non-goals:

* Do not rewrite the whole config loader.
* Do not migrate to a different config format.
* Do not change existing config semantics unless a bug is found.
* Do not add a full web UI.
* Do not require Astro to run during Rust tests.
* Do not introduce network access.
* Do not depend on platform-specific paths in generated output.

### Verification

Run:

```bat
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
```

If the normal target dir has access issues on Windows, use:

```bat
set CARGO_TARGET_DIR=%TEMP%\knightty-cargo-target
cargo clippy --workspace --all-targets
cargo test --workspace
```

### Acceptance criteria

* A default config can be generated from Rust defaults.
* Config reference metadata can be generated deterministically.
* Generated default config parses successfully.
* Tests catch drift between defaults, validation, and docs metadata.
* Existing config behavior remains compatible.
* Verification passes.
