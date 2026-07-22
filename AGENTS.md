# AGENTS.md

Guidance for AI coding agents working in this repo. Keep it lean; `CLAUDE.md` and
`docs/` hold the deep detail this file points to.

## What this is

Desktop app to translate RPG / visual-novel games by hand or via AI. **Tauri v2**
(Rust core) + **React / Vite / TypeScript**. The Rust side owns all heavy logic
(parse, extract, inject, DB, AI orchestration, keychain); the frontend is a thin
view over Tauri `invoke` commands + events. Eleven engines ship: **RPGMaker MV/MZ**
(JSON), **Ren'Py** (`.rpy`), **TyranoScript** (`.ks`), **KiriKiri** (`.ks`,
Shift-JIS/UTF-16), **Godot** (`.po`/`.csv`), **Unity Naninovel** (managed text +
compiled dialogue via bundled UnityPy helper), **Unity CSV localization**
(`StreamingAssets/Localization/<lang>/*.csv`), **Unity TextTable** (Mono
Addressables), **Hendrix** (MV/MZ with `game_messages.csv`), **Forger acod**
(Assassin's Creed Odyssey/Valhalla), and **ac-loctext** (Assassin's Creed Origins).

## Setup & commands

```bash
pnpm install                 # first-time setup
pnpm tauri dev               # run the app (hot-reload frontend + Rust)
pnpm build                   # frontend typecheck (tsc) + vite build
pnpm tauri build             # release binary + installer

cd src-tauri
cargo build                  # compile the Rust core
cargo test                   # all Rust tests (unit + integration)
cargo test --lib             # only lib unit tests (protect, ai::prompt)
cargo test roundtrip_identity        # a single test by name
cargo test --test extract_roundtrip  # one integration test file
```

## Gates — must pass before you're done

- **Rust builds warning-free.** Warnings are treated as breakage here.
- **`pnpm build` is clean.** `tsc` is strict with `noUnusedLocals` — no unused
  imports/vars/exports. There is no separate linter.
- **`cargo test` is green.** Every engine has a `roundtrip_identity` test; keep it.
- Run `cargo test` after any Rust change and `pnpm build` after any frontend change.

Dev API keys go in a `.env` (copy `.env.example`). `pnpm tauri dev` loads it in
debug builds; release builds ignore `.env` and read the OS keychain.

## Architecture

```
src-tauri/src/
  engine/    GameEngine trait + registry in mod.rs (engines() = detection order);
             hendrix, mvmz, renpy, kirikiri, tyrano, godot, unity (Naninovel),
             unity_csv, unity_textbl, forger_acod, ac_loctext; plus codes.rs,
             protect.rs, encoding.rs, rpa.rs (Ren'Py archive unpack),
             renpy_tl.rs (Ren'Py tl/<lang>/ fill), unrpyc.rs (decompile .rpyc)
  project/   SQLite store (db.rs), open/create + backup/export + export_mod (mod.rs)
  ai/        TranslationProvider trait; openai/anthropic/gemini; prompt + retry
  keys.rs    OS keychain (keyring) + .env fallback in debug builds
  lib.rs     #[tauri::command] surface + AI orchestration (translate_units)
src/         React UI; ipc.ts mirrors the command surface; Zustand stores split by
             concern (store.ts, settings.ts, translation.ts, errors.ts, ...)
```

## Invariants — do not break (read the file before changing these)

- **Pointer-addressed strings.** Every translatable string is located by an
  engine-opaque pointer: an RFC-6901 JSON Pointer (MvMz) or a `"start:len"` byte
  span (text engines). Only the owning engine interprets it. Never rewrite whole
  files by hand — go through the pointer.
- **Round-trip identity is a hard requirement.** `extract → inject with
  translation == source` must reproduce the original byte-for-byte.
- **Control codes are masked around AI, never sent raw.** `translate_units` calls
  `protect::mask_for(engine_id, …)` before a batch and `protect::restore` after; a
  unit whose sentinels don't restore is counted failed and **not written**.
- **Async commands must not hold the project lock across `.await`.**
  `AppState.project` guard is `!Send`: gather work under the lock, drop it, do all
  HTTP with no lock held, then re-lock briefly per batch to persist.
- **The grid is windowed.** The frontend never holds the whole unit list — only
  `total` + one `{offset, rows}` slice (~400 units). Live updates patch the window
  in place; never full-reload (it would jump the scroll).
- **serde field-name contract.** Structs with `#[serde(rename_all = "camelCase")]`
  have camelCase TS mirrors in `ipc.ts`; `TransUnit` fields are single words, no
  rename. Change a field name on both sides.
- **Secrets vs config split.** API keys live only in the OS keychain (`keys.rs`),
  loaded server-side; the frontend can set/check/clear but never read them back.
  Non-secret config lives in localStorage. Debug builds also read `.env`.
- **Game files are read-only until Export.** All state lives in the sidecar
  `<game>/.rpgtl/`. Export snapshots original bytes into `.rpgtl/source/` so
  re-export is idempotent.
- **Ren'Py exports as `tl/<lang>/`, not in place** (drives the game's bundled
  Ren'Py to generate the skeleton; source `.rpy` are never touched). Falls back to
  in-place inject when no bundled launcher is found. Compiled-only Ren'Py (no
  source `.rpy`) is auto-decompiled via the vendored `unrpyc` helper.
- **Two export modes.** `project::export` writes into the game (in-place).
  `project::export_mod` builds a staging mirror and zips it to
  `.rpgtl/mods/<lang>-<ts>.zip` — the game is never touched. Not available for
  Ren'Py or Hendrix (their export is already additive).
- **Tests copy the fixture; never dirty it.** Integration tests copy
  `tests/fixtures/mz-sample` to a temp dir before writing. Each engine has its own
  `tests/<engine>_roundtrip.rs`.

## Conventions

- Match the surrounding code's style, naming, and comment density.
- Commit messages / PRs / code comments: normal English, exact.
- Do **not** commit `.env` (gitignored; holds real provider API keys).
- Commit only when asked; push/merge/tag only when explicitly asked.

## Adding an engine

Implement `GameEngine` in `engine/<name>.rs`, register in `engine::engines()`
(order = detection priority; more specific first). Add a `mask_<name>` + `mask_for`
branch in `protect.rs`, mirror inline codes in `src/codes.ts` / `src/messageWidth.ts`.
Text formats reuse the byte-span pointer so `inject` just splices. Ship a fixture +
`tests/<name>_roundtrip.rs`. Nothing else changes — DB, commands, UI are
engine-agnostic. See `docs/ROADMAP.md`.

## Where to look

- `CLAUDE.md` — full rationale for each invariant, engine-by-engine detail.
- `README.md` — user-facing overview, providers, project layout.
- `docs/` — Obsidian vault; start at `docs/Home.md`. `docs/ENGINES.md`,
  `docs/ROADMAP.md`, `docs/QA-TEST-PLAN.md`, `docs/games/` (per-game research).
