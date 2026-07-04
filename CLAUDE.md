# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Desktop app to translate RPG / visual-novel games by hand or via AI. Five engines
ship: **RPGMaker MV/MZ** (JSON), **Ren'Py** (`.rpy`), **TyranoScript** (`.ks`,
UTF-8), **KiriKiri** (`.ks`, Shift-JIS/UTF-16), and **Godot** (gettext `.po` /
translation `.csv`). Tauri v2 (Rust core) + React/Vite/TypeScript. The Rust side owns all heavy logic (parse, extract, inject,
DB, AI orchestration, keychain); the frontend is a thin view over Tauri `invoke`
commands + events.

## Commands

```bash
pnpm install                 # first-time setup
pnpm tauri dev               # run the app (hot-reload frontend + Rust)
pnpm build                   # frontend typecheck (tsc) + vite build — run before tauri build
pnpm tauri build             # release binary + MSI/NSIS installer

cd src-tauri
cargo build                  # compile the Rust core
cargo test                   # all Rust tests (unit + integration)
cargo test roundtrip_identity            # a single test by name
cargo test --test extract_roundtrip      # one integration test file
cargo test --lib                         # only lib unit tests (protect, ai::prompt)
```

There is no linter config; `tsc` (strict, `noUnusedLocals`) is the frontend gate.
Rust must build warning-free.

## Architecture

Three Rust subsystems, each a module under `src-tauri/src/`, wired together by the
`#[tauri::command]` surface in `lib.rs`:

- **`engine/`** — the plugin seam. `GameEngine` trait (`detect`/`describe`/
  `extract`/`inject`), one impl per format: `mvmz.rs`, `renpy.rs`, `tyrano.rs`,
  `kirikiri.rs`, `godot.rs` (registered in `engines()`, tried in order). `codes.rs`
  maps RPGMaker event command codes (401 text, 102 choices, 320 name-change, …) to
  translatable parameter slots. `protect.rs` masks control/markup codes per
  engine (`mask_for(engine_id, …)`). `encoding.rs` is KiriKiri's Shift-JIS/UTF-16
  ↔ UTF-8 layer; KiriKiri reuses the TyranoScript KAG parser behind it. `godot.rs`
  handles gettext `.po` (`msgstr` in place, `msgid` as context) and Godot
  translation `.csv` (first locale column in place), both via the byte-span pointer.
- **`project/`** — SQLite persistence (`db.rs`) and project lifecycle (`mod.rs`):
  open/create the sidecar store, backup, and export.
- **`ai/`** — one `TranslationProvider` trait, providers behind it, plus prompt
  building and retry.

The frontend mirrors the command surface in `src/ipc.ts`; UI state is Zustand
(`src/store.ts` = project data, `src/settings.ts` = provider config in
localStorage).

## Invariants that span files (read before changing)

- **Pointer-addressed strings.** Every translatable string is a `TransUnit`
  (`model.rs`) located by an **engine-opaque** pointer: an RFC-6901 JSON Pointer
  for MvMz (`inject` writes via `serde_json::Value::pointer_mut`), a `"start:len"`
  byte span for the text engines (Ren'Py/Tyrano/KiriKiri splice the translation
  into that span). Only the owning engine interprets the pointer. Never rewrite
  whole files by hand — go through the pointer.
- **Round-trip identity is a hard requirement.** `extract → inject with
  translation == source` must reproduce the original. MvMz re-serializes
  **compact** (`serde_json` with `preserve_order`, UTF-8 not escaped — matches
  RPGMaker's own format); the text engines splice bytes so an unchanged unit is
  byte-identical with no re-serialize. KiriKiri only round-trips stateless
  encodings (UTF-8/UTF-16/Shift-JIS), so `encode(decode(bytes)) == bytes` holds;
  a translation unrepresentable in the source encoding (e.g. Thai in Shift-JIS)
  is emitted as UTF-16LE, which KiriKiri loads natively. Each engine has a
  `roundtrip_identity` test — keep them green.
- **Control codes are masked around AI, never sent raw.** The orchestrator in
  `lib.rs::translate_units` calls `protect::mask_for(engine_id, …)` (codes →
  `⟦n⟧` sentinels) before building a batch and `protect::restore` after. If
  `restore` reports a missing/mangled sentinel, that unit is counted failed and
  **not written** — do not store partially-restored text.
- **Async commands must not hold the project lock across `.await`.**
  `AppState.project` is `Mutex<Option<Project>>` and its guard is `!Send`.
  `translate_units` gathers work under the lock, drops it, does all HTTP with no
  lock held, then re-locks briefly per batch to persist. Follow this pattern for
  any new async command that touches both the DB and the network.
- **serde field-name contract.** Several structs use
  `#[serde(rename_all = "camelCase")]` (e.g. `ProjectInfo`, `Stats`,
  `DetectResult`, `ProviderConfig`, `Progress`, glossary/lint results); their
  TS mirrors in `ipc.ts` use camelCase. `TransUnit` fields are single words with
  no rename. Changing a field name means changing both sides.
- **Secrets vs config split.** API keys live only in the OS keychain
  (`keys.rs`, `keyring`), keyed by provider kind, and are loaded server-side in
  the command; the frontend can set/check/clear but never read them back.
  Non-secret provider config lives in localStorage. **Debug builds only**,
  `keys::get_key` first checks an env var (`RPGTL_KEY_<KIND>` or the provider's
  conventional name) and `run()` loads a `.env` via `dotenvy`, so `pnpm tauri
  dev` can use a shell/`.env` key without the keychain; release ignores both.
- **Game files are read-only until Export.** All state is kept in the sidecar
  `<game>/.rpgtl/` (`project.db`, backups). `project::export` backs up the files
  it will touch, then injects in place.

## Adding an engine

Implement `GameEngine` in a new `engine/<name>.rs`, register it in
`engine::engines()` (order = detection priority; more specific first). Add a
`mask_<name>` + `mask_for` branch in `protect.rs` and mirror its inline codes in
`src/codes.ts`/`src/messageWidth.ts`. Text formats reuse the byte-span pointer
(`"start:len"`) so `inject` just splices — no re-serialize. Ship a fixture +
`tests/<name>_roundtrip.rs` (detect, extract-vs-code, round-trip identity,
targeted inject). Nothing else changes — the DB, commands, and UI are
engine-agnostic. See `docs/ROADMAP.md` for the full pattern and next targets.

## AI providers

`ai::make_provider` dispatches on `ProviderConfig.kind`. `openai.rs` serves
`openai` / `openrouter` / `local` (all OpenAI-compatible, differ by base URL +
auth); `anthropic.rs` and `gemini.rs` have their own wire formats. Batches use a
numbered JSON array so `prompt::parse_batch_response` can re-align by index;
`ai::translate_batch_or_split` falls back to per-item requests when a batch
response can't be aligned. `list_models` powers the model picker (notably
Ollama's installed models via its OpenAI-compatible `/models`).

## Tests

Rust tests run against a synthetic MZ game in `src-tauri/tests/fixtures/mz-sample`
(no real game needed). Integration tests copy the fixture to a temp dir before
writing, so they never dirty it. The fixture intentionally contains two units
with source `"Yes"` — the TM test relies on that duplicate. Each engine also has
its own `tests/<engine>_roundtrip.rs`; Ren'Py/Tyrano use committed fixtures while
KiriKiri builds its Shift-JIS/UTF-16 fixtures in-test (real bytes needed).
