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
