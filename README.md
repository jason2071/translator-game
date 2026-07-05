# RPGMaker Translator

Desktop app to translate RPG / visual-novel games — **RPGMaker MV/MZ**,
**Ren'Py**, **TyranoScript**, **KiriKiri**, and **Godot** (`.po`/`.csv`) — with an
engine-plugin architecture ready for more (VX Ace, 2000/2003, HTML/Twine, …).
Translate by hand or with AI (Local / Claude / OpenAI / Gemini / OpenRouter / any
OpenAI-compatible endpoint).

Built with **Tauri v2** (Rust core) + **React + Vite + TypeScript**.

## What it does

Project-based workflow — the original game is never touched until you export:

1. **Import** a game folder → the engine is auto-detected and every
   translatable string is extracted into a grid.
2. **Translate** by hand (inline editing) or with AI (per-file, all-untranslated,
   or the current filter).
3. **Export** → the translations are patched back into the game's data files,
   with an automatic backup of every file that changes. For **RPGMaker**, an
   optional *Embed Thai font* step drops a Thai-capable font (Sarabun) into the
   game and repoints its fonts at it — MV via `fonts/gamefont.css`, MZ via
   `System.json` `mainFontFilename` — so translated Thai renders instead of
   missing-glyph boxes. **Ren'Py** games export the native way instead — the app
   drives the game's own bundled Ren'Py to generate `game/tl/<language>/` files and
   fills them in, so the original scripts are never modified, nothing recompiles,
   and the translation becomes a selectable in-game language (with the same font
   drop-in so it renders).

Features: a **windowed** grid that stays light on huge projects (holds only the
visible slice — scales to ~1M strings), translation memory (auto-fills
duplicate/identical strings), glossary + consistency lint, engine-aware code
protection (RPGMaker `\C[n]`, `%1`, Ren'Py `[tag]`/`{tag}`, KAG `[tag]`) so AI
can't corrupt markup, batch translation with rate-limit + retry, a grid that fills
row-by-row live as results land, re-translate at any scope (whole project / file /
single line / overwrite existing), auto-copy of untranslatable sources (numbers,
punctuation-only) so they don't count as failures, an **Errors** panel that lists
which units failed and why, independent AI providers for Run vs glossary
suggestion, a **Thinking / reasoning** toggle you can switch off per provider
(Ollama / OpenRouter / Gemini) so reasoning models don't burn the token budget,
and API keys stored in the OS keychain (never on disk).

## Architecture

```
src-tauri/src/
  engine/        GameEngine trait + registry; mvmz / renpy / tyrano / kirikiri /
                 godot, codes.rs, protect.rs, encoding.rs (KiriKiri Shift-JIS/UTF-16)
  project/       SQLite store (db.rs), open/create + backup/export (mod.rs)
  ai/            TranslationProvider trait; openai/anthropic/gemini; prompt + retry
  keys.rs        OS keychain (keyring)
  lib.rs         Tauri command surface + AI orchestration
src/             React UI (ImportView, GridView, TranslateBar, SettingsView, Glossary)
```

Each translatable string is located by an **engine-opaque pointer** — an RFC-6901
JSON Pointer for the JSON engine, a byte span for the text engines — so injection
rewrites exactly that node/span and nothing else. A per-engine round-trip test
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

For dev you can supply provider API keys via a `.env` (copy `.env.example`) — it
is loaded only in debug builds, so `pnpm tauri dev` picks up `RPGTL_KEY_OPENAI`
etc. without touching the OS keychain. Release builds ignore `.env`.

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
4. In the **AI translate** bar pick the provider + scope and **Run** (cancellable;
   the grid fills row-by-row as batches land). Toggle **Overwrite existing** to
   re-translate rows that already have a translation; **Retry failed** re-runs only
   the failures, and the **Errors (N)** button opens a panel listing each failed
   unit's file, source, and reason. **Apply TM** fills duplicates for free.
5. **Export → game** when done (auto-backup to `.rpgtl/backups/<timestamp>/`).

### Providers

| Provider   | Endpoint                              | Key |
|------------|---------------------------------------|-----|
| Local      | OpenAI-compatible (Ollama :11434/v1)  | no  |
| OpenAI     | `/v1/chat/completions`                | yes |
| OpenRouter | `openrouter.ai/api/v1`                | yes |
| Claude     | `api.anthropic.com/v1/messages`       | yes |
| Gemini     | `…:generateContent`                   | yes |

The Local / OpenAI / OpenRouter kinds take a custom **Base URL**, so any
OpenAI-compatible gateway works — e.g. OpenCode Zen (`opencode.ai/zen/v1`),
Ollama Cloud, or LM Studio. Use **Refresh** to pull the endpoint's model list.

For reasoning models, switching **Thinking / reasoning** off stops the model
burning its token budget on hidden thoughts before answering — it maps to the
right knob per provider (Ollama's native `/api/chat` `think:false`, OpenRouter
`reasoning.enabled=false`, Gemini `thinkingBudget=0`). Run and glossary suggestion
each have their own provider selector, so you can pair a cheap local model for
glossary with a stronger one for the full Run.

## Project data

A sidecar `.rpgtl/` folder is created next to the game:
`project.db` (units, translation memory, glossary), `config.json`, and
`backups/`. Delete it to start over; the game files themselves are only changed
on **Export**.

## Roadmap

- More engines: HTML (Twine/SugarCube), VX Ace/VX/XP (`.rvdata2`), RPGMaker
  2000/2003 (LCF binary), Wolf RPG. See `docs/ROADMAP.md`.
- Fuzzy translation memory, per-run token/cost estimate, multi-select in the grid.
