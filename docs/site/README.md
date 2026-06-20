# Knightty Docs

Knightty user documentation built with Astro and Starlight.

## Commands

Run commands from `docs/site`.

| Command            | Action                                           |
| :----------------- | :----------------------------------------------- |
| `pnpm install`     | Installs dependencies                            |
| `pnpm dev`         | Starts local dev server at `localhost:4321`      |
| `pnpm astro check` | Runs Astro type/content checks                   |
| `pnpm build`       | Builds the production site to `./dist/`          |
| `pnpm astro ...`   | Run CLI commands like `astro add`, `astro check` |

## Config Reference

`src/lib/config-reference.ts` reads `../generated/config-reference.json` first and falls back to
`src/data/config-reference.fixture.json` while generated data does not exist.

Generate the Rust-backed reference artifacts from the workspace root:

```bash
cargo run -p xtask -- generate-config-docs
```

The same Rust source can print the default TOML user config:

```bash
cargo run -p app -- +print-default-config
```

Knightty reads TOML from `knightty.config`; the old `config.json` path is not used.
