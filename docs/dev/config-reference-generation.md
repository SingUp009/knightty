# Config Reference Generation

Knightty configuration reference data is generated from Rust metadata.

## Source of truth

- Runtime config structs and validation live in `crates/app/src/config.rs`.
- Documentation metadata lives in `crates/app/src/config_spec.rs`.
- The user config file is TOML named `knightty.config`.
- `config.json` is no longer read.

Default config search order:

1. `KNIGHTTY_CONFIG`
2. `%APPDATA%\knightty\knightty.config` on Windows
3. `$XDG_CONFIG_HOME/knightty/knightty.config` on Unix
4. `~/.config/knightty/knightty.config` on Unix

## Regenerate artifacts

From the workspace root:

```bash
cargo run -p xtask -- generate-config-docs
```

This writes:

- `docs/generated/config-reference.json`
- `docs/generated/default-config.toml`

To print the default config without writing files:

```bash
cargo run -p app -- +print-default-config
```

## Astro consumption

The Astro site reads `docs/generated/config-reference.json` first. If the generated file does not exist, `docs/site/src/data/config-reference.fixture.json` is used as temporary fallback data.

The generated JSON keeps the existing Astro shape:

- `key`
- `category`
- `type`
- `default`
- `description`
- `examples`
- `validValues`
- `range`
- `reload`
- `platform`
- `security`
- `since`
- `deprecated`

## Adding settings

When adding a user-facing setting:

1. Add the field and validation in `crates/app/src/config.rs`.
2. Add the key to `SUPPORTED_CONFIG_KEYS`.
3. Add metadata in `crates/app/src/config_spec.rs`.
4. Regenerate artifacts with `cargo run -p xtask -- generate-config-docs`.
5. Run Rust tests so drift checks catch missing metadata or stale generated files.
