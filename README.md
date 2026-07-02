# RPGMaker Translator

Desktop app to translate RPGMaker games — **RPGMaker MV/MZ** in V1, with an
engine-plugin architecture ready for VX Ace, 2000/2003, Ren'Py, Wolf RPG, etc.
Translate by hand or with AI (Local / Claude / OpenAI / Gemini / OpenRouter).

Built with **Tauri v2** (Rust core) + **React + Vite + TypeScript**.

## What it does

Project-based workflow — the original game is never touched until you export:

1. **Import** a game folder → the engine is auto-detected and every
   translatable string is extracted into a grid.
2. **Translate** by hand (inline editing) or with AI (per-file, all-untranslated,
   or the current filter).
3. **Export** → the translations are patched back into the game's data files,
   with an automatic backup of every file that changes.

Features: virtualized grid for 10k+ strings, translation memory (auto-fills
duplicate/identical strings), glossary + consistency lint, RPGMaker control-code
protection (`\C[n]`, `\V[n]`, …) so AI can't corrupt them, batch translation with
rate-limit + retry, and API keys stored in the OS keychain (never on disk).

## Architecture

```
src-tauri/src/
  engine/        GameEngine trait + registry; mvmz.rs (MV/MZ), codes.rs, protect.rs
  project/       SQLite store (db.rs), open/create + backup/export (mod.rs)
  ai/            TranslationProvider trait; openai/anthropic/gemini; prompt + retry
  keys.rs        OS keychain (keyring)
  lib.rs         Tauri command surface + AI orchestration
src/             React UI (ImportView, GridView, TranslateBar, SettingsView, Glossary)
```

Each translatable string is located by an **RFC-6901 JSON Pointer** into its
file, so injection rewrites exactly that node and nothing else. A round-trip test
(`extract → inject source==translation → compare`) guarantees no structural loss.

**Adding an engine** = implement `GameEngine` in a new file and list it in
`engine::engines()`. Nothing else changes.

## Prerequisites

- [Rust](https://rustup.rs) (stable) + a C toolchain (MSVC on Windows — bundled
  SQLite compiles from source)
- Node 18+ and `pnpm`
- Tauri v2 system deps: see <https://tauri.app/start/prerequisites/>

## Develop

```bash
pnpm install
pnpm tauri dev      # launches the app with hot-reload
```

## Build a release / installer

```bash
pnpm tauri build    # → src-tauri/target/release/ + bundled installer
```

## Test

```bash
cd src-tauri
cargo test          # engine extraction, round-trip, project flow, TM/glossary,
                    # control-code protection, AI prompt/parse
```

Tests run against a synthetic MZ fixture in `src-tauri/tests/fixtures/mz-sample`.

## Using AI translation

1. Open **⚙ (provider) → Settings**, pick a provider, set the model, and paste
   the API key (stored in the OS keychain — Local/Ollama needs no key).
2. Optionally set tone, an extra prompt, batch size, and a rate limit.
3. Add **Glossary** terms (proper nouns, stats) for consistency.
4. In the **AI translate** bar choose a scope and **Run** (cancellable, with a
   live progress bar). **Apply TM** fills duplicates for free.
5. **Export → game** when done (auto-backup to `.rpgtl/backups/<timestamp>/`).

### Providers

| Provider   | Endpoint                              | Key |
|------------|---------------------------------------|-----|
| Local      | OpenAI-compatible (Ollama :11434/v1)  | no  |
| OpenAI     | `/v1/chat/completions`                | yes |
| OpenRouter | `openrouter.ai/api/v1`                | yes |
| Claude     | `api.anthropic.com/v1/messages`       | yes |
| Gemini     | `…:generateContent`                   | yes |

## Project data

A sidecar `.rpgtl/` folder is created next to the game:
`project.db` (units, translation memory, glossary), `config.json`, and
`backups/`. Delete it to start over; the game files themselves are only changed
on **Export**.

## Roadmap

- More engines: VX Ace/VX/XP (`.rvdata2`), RPGMaker 2000/2003 (LCF binary),
  Ren'Py, Wolf RPG, Unity.
- Fuzzy translation memory, per-run token/cost estimate, multi-select in the grid.
