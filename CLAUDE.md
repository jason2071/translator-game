# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Desktop app to translate RPG / visual-novel games by hand or via AI. Seven engines
ship: **RPGMaker MV/MZ** (JSON), **Ren'Py** (`.rpy`), **TyranoScript** (`.ks`,
UTF-8), **KiriKiri** (`.ks`, Shift-JIS/UTF-16), **Godot** (gettext `.po` /
translation `.csv`), **Unity (Naninovel)** (managed text — UI / names / gallery
— **and** the compiled story dialogue, both via a bundled UnityPy helper), and
**Unity (CSV localization)** (a different Unity storage method — plaintext
`StreamingAssets/Localization/<lang>/*.csv` catalogs in IL2CPP + Addressables games,
translated into a new locale folder + a Thai-font swap into the game's font bundle).
Tauri v2 (Rust core) + React/Vite/TypeScript. The Rust side owns all heavy logic (parse, extract, inject,
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
  `extract`/`inject`, plus optional `stale_companions`/`embed_font`), one impl per
  format: `mvmz.rs`, `renpy.rs`, `tyrano.rs`, `kirikiri.rs`, `godot.rs` (registered
  in `engines()`, tried in order). `embed_font` (opt-in at export, RPGMaker only so
  far) drops the shared bundled Thai font `engine::TARGET_FONT` (Sarabun, OFL) into
  the game and repoints its fonts at it — MV rewrites `fonts/gamefont.css`, MZ sets
  `System.json` `advanced.mainFontFilename` — so translated Thai isn't tofu; Ren'Py
  does the equivalent remap inside its own `tl/` path. For RPGMaker it also installs
  a tiny `RPGTL_ThaiText` plugin (registered last in `js/plugins.js`) that thins the
  text outline, so Thai's stacked tone/vowel marks don't blob under RPGMaker's thick
  default stroke. `codes.rs`
  maps RPGMaker event command codes (401 text, 102 choices, 320 name-change, …) to
  translatable parameter slots. `protect.rs` masks control/markup codes per
  engine (`mask_for(engine_id, …)`). `encoding.rs` is KiriKiri's Shift-JIS/UTF-16
  ↔ UTF-8 layer; KiriKiri reuses the TyranoScript KAG parser behind it. `godot.rs`
  handles gettext `.po` (`msgstr` in place, `msgid` as context) and Godot
  translation `.csv` (first locale column in place), both via the byte-span pointer.
  `renpy_tl.rs` + `renpy::export_tl` are the **Ren'Py `tl/<lang>/` export path**
  (see the export invariant below): rather than splice, run the game's own bundled
  Ren'Py to generate the translation skeleton, then fill it — source `.rpy` are
  never touched. **Compiled-only Ren'Py games** (ship `.rpyc`/`.rpa` with no source
  `.rpy`) are handled at import by `renpy::ensure_decompiled`: it stages `.rpyc` out
  of any `.rpa` (`rpa::extract_rpyc`), finds the game's own bundled Python
  (`<root>/lib/py{3,2}-*` **or** a bare `<os>-<arch>` dir, major version selects the
  branch), and runs the embedded `engine::unrpyc` decompiler (MIT, vendored under
  `resources/unrpyc/` as v2 for Py3 / v1 for Py2, materialized to a temp cache) to
  write `.rpy` in place — then the normal flow reads them. Invoked with cwd = the
  unrpyc dir + a relative `unrpyc.py` so its sibling `import decompiler` resolves
  (the bundled Ren'Py Python doesn't add an absolute script's dir to `sys.path`). If
  no Python is found or unrpyc fails, import degrades to the original actionable
  "decompile with unrpyc" error — never a silent empty project. `unity.rs` is the
  **Unity (Naninovel)** engine, via a **UnityPy helper** (`resources/unity/rpgtl_unity.py`)
  driven like unrpyc; two tiers. **Tier 1 — managed text** (UI, names, gallery):
  `TextAsset` `Key: Value` docs, pointer `"<file>#<pathId>#<key>"`. **Tier 2 —
  compiled story dialogue**: the `Naninovel.Script` MonoBehaviours are
  stripped-typetree with `[SerializeReference]` script-lines UnityPy can't read
  structurally, but the spoken text is plain length-prefixed UTF-8 in the raw blob, so
  the helper enumerates it at 4-byte-aligned offsets and **splices on the bytes** (no
  typetree), pointer `"dlg#<file>#<pathId>#<idx>"` (idx into a deterministic
  enumeration). Both tiers are **locale-aware**: they target the game's `en`
  localization (Naninovel `translate` docs / per-script `zh→en` localization MBs), so
  a Thai user translates the existing **English → Thai**. Round-trip relaxed to
  **load-faithful** (like KiriKiri's UTF-16 exception, since UnityPy re-serializes the
  whole `SerializedFile`). Detects a `<name>_Data/` dir with `resources.assets` + a
  `*Naninovel*.dll`, declining plain Unity games. Unity games ship no Python, so the release build embeds a **frozen exe**
  (`rpgtl-unity.exe`, PyInstaller via `scripts/freeze-unity-sidecar.ps1`,
  `include_bytes!`d through `build.rs` — a git-ignored artifact, `cargo build`
  succeeds without it); a build lacking the exe falls back to the system `python` +
  the plain script. `unity_csv.rs` is a **second, unrelated Unity engine** —
  **Unity (CSV localization)** (id `unity-csvloc`) — for IL2CPP + Addressables games
  (e.g. Milfarion/Texic's Milf Plaza) that keep all text in plaintext
  `StreamingAssets/Localization/<lang>/*.csv` catalogs (`;`-delimited `key;value`,
  values never quoted / never containing `;`, each locale folder with a `meta.txt`
  `{"_visibleName":…}`). The game **folder-scans** `Localization/` to build its
  language menu, so export is **additive** (like Ren'Py's `tl/`): `export_locale`
  writes a new `<lang>/` locale folder (source locales untouched) that becomes an
  in-game selectable language. Pointer is the value's **byte span** `"start:len"`
  into the source-locale CSV (Godot-style splice) → true byte-identity round-trip.
  **Fonts** are the hard part: the stock TMPro fonts have no Thai, but every font
  chains to a **Dynamic-atlas** TMP_FontAsset (`m_AtlasPopulationMode == 1`) that
  rasterizes glyphs at runtime from an in-bundle source `Font`, so `embed_font` swaps
  that Font's TTF for the bundled Sarabun via the shared `rpgtl_unity.py`
  **`swap-font`** command (no SDF atlas baking) and then zeroes the bundle's CRC in
  `catalog.bin` — a pure-Rust byte patch (locate the 16-byte content hash from the
  bundle filename → `Crc u32` at hash offset **+60** → `0`), without which
  Addressables' CRC check rejects the modified bundle and hangs the game at load.
- **`project/`** — SQLite persistence (`db.rs`) and project lifecycle (`mod.rs`):
  open/create the sidecar store, backup, and export.
- **`ai/`** — one `TranslationProvider` trait, providers behind it, plus prompt
  building and retry.

The frontend mirrors the command surface in `src/ipc.ts`; UI state is Zustand,
split by concern: `src/store.ts` = project data + the grid **window** (see the
windowed-loading invariant), `src/settings.ts` = provider config in localStorage,
`src/translation.ts` = the shared Run/glossary job queue + live progress,
`src/errors.ts` = per-unit failure reasons (fed by `translate://failed`),
`src/glossarySuggest.ts` = glossary candidate rows, `src/recents.ts` = recent
projects.

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
  **not written** — do not store partially-restored text. After a successful
  restore, into a **non-CJK target** (`is_cjk_lang` — not zh/ja/ko) the stored text
  is run through `protect::normalize_cjk_brackets`, which rewrites CJK bracket
  punctuation (`「」『』【】〔〕〈〉《》（）`) to ASCII parens `( )` — the bundled Thai
  font can't render them, and parens are safe in every engine (unlike `[ ]` in Ren'Py
  or `{ }` in TMPro). Applies to new translations only.
- **Async commands must not hold the project lock across `.await`.**
  `AppState.project` is `Mutex<Option<Project>>` and its guard is `!Send`.
  `translate_units` gathers work under the lock, drops it, does all HTTP with no
  lock held, then re-locks briefly per batch to persist. Follow this pattern for
  any new async command that touches both the DB and the network.
- **The grid is windowed — the frontend never holds the whole unit list.** A
  project can be ~1M units, so `store.ts` keeps only `total` (the virtualizer's
  row count, from `count_units`) plus one `window` — a `{ offset, rows }` slice of
  ~400 units around the scroll position. `ensureWindow(start,end)` refetches (via
  `list_units` offset/limit) once the visible range comes within `MARGIN` of the
  slice edge; a module-level `winReq` counter drops stale fetches so an old
  filter/scroll never overwrites a newer window. Live Run updates
  (`translate://units`) patch `window.rows` in place — never a full reload (which
  would jump the scroll). The backend was already 1M-ready (indexed, `ORDER BY
  id`, offset/limit); a full RAM-streaming translate loop is the deferred piece.
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
  `<game>/.rpgtl/` (`project.db`, backups, `source/`). `project::export` backs up
  the files it will touch, then injects in place.
- **Two export modes: in-place vs mod.** `project::export` writes the translation
  **into the game** (backup + in-place inject + optional `embed_font`).
  `project::export_mod` instead builds a **staging mirror** of the game root, writes
  only the changed/added files there, and zips it to `.rpgtl/mods/<lang>-<ts>.zip`
  (via `zip_dir`) — a distributable overlay the user copies over their game, with the
  **game never modified**. The redirect rides the existing `inject(root, units,
  out_dir)` seam plus an `out_dir`/`out_base` on the font + additive-export paths
  (`GameEngine::embed_font` gained an `out_dir` where patched/new files land — reads
  come from `data_dir`, or from `out_dir` if a file was already injected there;
  `unity_csv::export_locale` gained an `out_base`). A mod is built to be the target
  language **without an in-game language switch**: `unity-csvloc` overwrites *every*
  shipped source locale by key (keys are shared across locales), single-locale
  engines are inherently the target. Coverage: `unity-csvloc` (overwrite-all-locales)
  and every single-locale text/JSON engine (`rpgmaker-mvmz`, `godot`, `tyrano`,
  `kirikiri`, `forger-acod`, `ac-loctext`) via `build_mod_via_inject` — which injects
  from a **pristine read-root** (`pristine_read_root`: per touched file, the
  `.rpgtl/source/` snapshot if present, else the live game) so a byte-span splice stays
  valid even after a prior in-place export. **Ren'Py** and **Hendrix** are export-to-game
  only (they build the translation *additively into the game* — Ren'Py runs the game's
  own Ren'Py to write `tl/<lang>/`, Hendrix appends a column + registers the language —
  so a "game untouched" mod isn't available; their in-place output is already an
  additive overlay).
- **Export must be idempotent.** A `pointer` is a byte offset into the *original*
  file, but export injects in place, so a naive second export would splice
  original offsets into already-translated bytes — cutting multi-byte characters
  (invalid UTF-8) and doubling text. `project::export` therefore snapshots each
  file's original bytes into `.rpgtl/source/` on first export and restores from it
  before every later injection, so re-export reproduces the same output. The
  snapshot is seeded from the earliest `.rpgtl/backups/<ts>/` copy when present
  (so a project exported before this scheme still snapshots ORIGINAL bytes and
  self-repairs on its next export). `tests/reexport_idempotent.rs` guards this.
- **Ren'Py exports as `tl/<lang>/`, not in place.** Splicing a `.rpy` changes it,
  so Ren'Py recompiles it and re-parses the game's creator-defined statements
  under the runtime Ren'Py — which surfaces game-vs-version incompatibilities
  (recompile is decided by an MD5 of the `.rpy` content, `renpy/script.py`) and can
  crash at load. So for Ren'Py, `project::export` calls `renpy::export_tl` first:
  it finds the game's bundled Ren'Py launcher (`<name>.exe` beside `<name>.py` +
  `renpy/`), runs `<exe> <root> translate <lang>` to generate the skeleton (whose
  identifiers are then guaranteed correct — `renpy_tl.rs` only *validated* this
  algorithm, it doesn't compute the ids used in production), fills it from the DB
  via `renpy_tl::fill_tl` (match source string → translation; dialogue = the say
  line's last quoted string, `strings` block = `old`/`new`), and writes a global
  `zzz_translator.rpy` (default `config.language` + a language-scoped font remap so
  Thai isn't "NO GLYPH"). The source `.rpy` are never touched → no recompile, and
  `<lang>` becomes a selectable in-game language. Falls back to in-place inject
  when no bundled launcher is found.

## Adding an engine

Implement `GameEngine` in a new `engine/<name>.rs`, register it in
`engine::engines()` (order = detection priority; more specific first). Add a
`mask_<name>` + `mask_for` branch in `protect.rs` and mirror its inline codes in
`src/codes.ts`/`src/messageWidth.ts`. Text formats reuse the byte-span pointer
(`"start:len"`) so `inject` just splices — no re-serialize. Ship a fixture +
`tests/<name>_roundtrip.rs` (detect, extract-vs-code, round-trip identity,
targeted inject). Nothing else changes — the DB, commands, and UI are
engine-agnostic. See `docs/ROADMAP.md` for the full pattern and next targets.

Per-game/engine research lives in `docs/games/`. Assassin's Creed (AnvilNext) has
two shipped engines, both fed by external community tools:
`engine/forger_acod.rs` (`.acod` UTF-16LE string tables the Forger tool exports —
Odyssey/Valhalla; see `docs/games/anvilnext-forger.md`) and `engine/ac_loctext.rs`
(**Origins**, which ships no `.acod`: the `aclocexport`/`aclocimport` pair turns
its binary `.Localization_Package` into UTF-8 `Id: [0x…]` text this engine
translates; see `docs/games/anvilnext-locpackage-format.md`).

## Docs (Obsidian vault)

`docs/` is both the project documentation and an **Obsidian vault** — open it in
Obsidian and start at `docs/Home.md` (the map-of-content / MOC). When adding or
editing docs, follow the vault conventions so the graph view, backlinks, and tag
search keep working:

- **Folders by topic**, each with a **folder note** of the same name as its index —
  e.g. deep-dive research lives under `docs/games/` and `docs/games.md` is its index.
- **YAML frontmatter** on every note: `title`, `aliases`, `tags`, `created`, and a
  `status` where relevant (`proposed` / `planned` / `implemented`).
- **Wikilinks** (`[[note]]`) between notes, not raw paths, so backlinks resolve.
- **Nested tag taxonomy**: `type/research`, `engine/<name>`, `game/<name>`, `moc`.
- **Stable core docs.** `ENGINES.md`, `ROADMAP.md`, `QA-TEST-PLAN.md` keep their
  names and top-level paths — they're referenced from this file and `README.md`;
  don't move or rename them.
- **Not committed / regenerable:** `.obsidian/` (per-user vault config) and
  `graphify-out/` (the `/graphify` knowledge-graph output) are both git-ignored;
  the Markdown notes are tracked.

## AI providers

`ai::make_provider` dispatches on `ProviderConfig.kind`. `openai.rs` serves
`openai` / `openrouter` / `local` (all OpenAI-compatible, differ by base URL +
auth); `anthropic.rs` and `gemini.rs` have their own wire formats. Batches use a
numbered JSON array so `prompt::parse_batch_response` can re-align by index;
`ai::translate_batch_or_split` falls back to per-item requests when a batch
response can't be aligned. `list_models` powers the model picker (notably
Ollama's installed models via its OpenAI-compatible `/models`).

**Disabling thinking/reasoning is per-provider** (the **Thinking / reasoning**
toggle in Settings, off ⇒ `req.thinking = Some(false)`; each provider must
translate that to its own wire knob — there is no portable flag):
- **Local (Ollama):** the OpenAI-compatible `/v1` endpoint ignores `think:false`
  (reasoning still eats the `max_tokens` budget → empty content), so `openai.rs`
  appends `/no_think` to the user turn and prefers a native `/api/chat` call
  (`ollama_chat`: rewrites `/v1`→`/api/chat`, sends `think:false` +
  `options.num_predict`, reads `message.content`), falling back to `/v1` only if
  that fails.
- **OpenRouter:** `body["reasoning"] = {"enabled": false}` (works on hybrid
  models; mandatory-reasoning models like `deepseek-r1` 400 — leave the toggle
  off for those).
- **Gemini:** `generationConfig.thinkingConfig.thinkingBudget = 0`.
- **Anthropic / OpenAI:** no change sent (non-thinking by default for the models
  used).
Because a reasoning model with too small a budget returns empty content (looks
like "no JSON array found"), `test_provider` sizes its probe with
`config.max_tokens()`, not a fixed 256.

The **Run provider** (`settings.active`) and the **glossary provider**
(`settings.glossaryProvider`) are independent selectors — glossary suggestion can
use a different/cheaper model than a full Run. Each `ProviderConfig` is stored
per-kind (`configFor(kind)`); the Settings modal edits any kind via its own local
`editing` state without changing which one Run uses.

## Tests

Rust tests run against a synthetic MZ game in `src-tauri/tests/fixtures/mz-sample`
(no real game needed). Integration tests copy the fixture to a temp dir before
writing, so they never dirty it. The fixture intentionally contains two units
with source `"Yes"` — the TM test relies on that duplicate. Each engine also has
its own `tests/<engine>_roundtrip.rs`; Ren'Py/Tyrano use committed fixtures while
KiriKiri builds its Shift-JIS/UTF-16 fixtures in-test (real bytes needed).
