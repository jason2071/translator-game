# Roadmap — next engines + backlog

The app translates games by hand or AI. It currently ships **two engines** —
RPGMaker MV/MZ (JSON) and Ren'Py (`.rpy`) — as of **v0.3.0** (in-app auto-update
live). This document captures the proven engine-adding pattern, a recommended
next engine, ranked alternatives, and independent backlog items, so work can
resume in one step.

Nothing here is started yet. The engine-adding pattern below is the same
regardless of which target is chosen next.

## The engine-adding pattern (proven with Ren'Py — reuse it)
Adding an engine touches only the engine seam; model/DB/UI are engine-agnostic.
- Implement `GameEngine` in a new `src-tauri/src/engine/<name>.rs`; register it in
  `engine::engines()` in `src-tauri/src/engine/mod.rs` (detection order: specific
  first). Trait = `detect` / `describe` / `extract` / `inject` (+ optional
  `stale_companions`).
- `TransUnit.pointer` (`src-tauri/src/model.rs`) is an **engine-opaque string** —
  JSON Pointer for MvMz, `"start:len"` byte span for Ren'Py. A text engine reuses
  the byte-span approach: `inject` reads the file bytes and splices each
  translation into its span (sorted by descending offset), so
  `translation == source` round-trips **byte-identical** with no re-serialize.
  See `renpy.rs::inject` / `parse_pointer`.
- Code masking: add `mask_<engine>` in `src-tauri/src/engine/protect.rs` and a
  branch in `protect::mask_for(engine_id, text)` (shared `restore` is already
  engine-agnostic). Mirror the engine's inline codes in `src/codes.ts`
  (`codesMismatch`) for the grid warning.
- Skip derived / other-language files so they aren't imported as source (Ren'Py
  skips `game/tl/<lang>/` in the dir walk and invalidates `.rpyc` via
  `stale_companions`). Apply the equivalent for the new format.
- Tests: fixture at `src-tauri/tests/fixtures/<name>-sample/`, integration
  `src-tauri/tests/<name>_roundtrip.rs` (detect, extract-vs-code, byte round-trip,
  targeted inject), plus unit tests in the engine module. `cargo test --lib`
  works even when a dev app holds the main-exe lock; the integration binary needs
  the dev app stopped.
- The dev harness (`src-tauri/examples/harness.rs`) is already engine-aware:
  `extract <game>` prints the breakdown + verifies round-trip; `ai <game> <model>
  <n>` translates via Ollama with the right masking.

## Recommended next engine: TyranoScript / KiriKiri (`.ks` text)
Text-based, so it reuses the byte-span locator + protect pattern from Ren'Py at
moderate effort, and covers a large JP visual-novel catalog (audience overlaps
our users). Lower risk than any binary format.
- **detect**: a `data/` or `scenario/` tree of `.ks` files (Tyrano: `data/scenario`).
- **extract**: text lives between KAG tags — segments that aren't tags or labels.
  Skip `[macro]`/`[jump]`/`[eval]`/`*labels`/`@`-commands; capture narrative text
  and `[ptext]`/message text. Speaker often via `[chara_...]` or a `#name` line →
  context.
- **locator**: byte span (same as Ren'Py).
- **protect** (`mask_tyrano`): KAG tags `[...]`, `%variables`, ruby `[ruby ...]`,
  and `&`-entities.
- **Main new challenge — encoding**: KiriKiri scripts are frequently Shift-JIS or
  UTF-16; Tyrano is usually UTF-8. Detect the file encoding, decode to UTF-8 for
  editing, and re-encode to the original on inject so round-trip stays byte-exact.
  This is the one piece Ren'Py did not need (its `.rpy` are UTF-8).

## Alternatives
- **RPGMaker VX Ace / VX / XP** — same audience as the flagship MV/MZ (largest JP
  RPG catalog). Blocker: `Data/*.rvdata2` is a **Ruby Marshal** binary dump; no
  mature Rust crate, so it needs a hand-rolled Marshal reader + writer with
  byte-exact round-trip. Highest audience value, **largest effort/risk** (multi-week).
- **Godot** — `.po` / `.csv` are trivial (gettext); `.translation` is binary. Low
  effort but niche unless the game ships `.po`.
- **Others** (later): RPGMaker 2000/2003 (liblcf, C++), Wolf RPG (`.mps`, often
  encrypted), Unity (IL2CPP/Mono/TextMeshPro — XUnity's domain), Unreal (`.locres`).

## Small backlog (independent, quick — do alongside or between engines)
- **Engine-aware overflow default**: the message-width guard default 46 is
  RPGMaker-tuned; Ren'Py auto word-wraps so it over-warns. In
  `src/messageWidth.ts` / the import flow, default to 46 for RPGMaker and 0/high
  for Ren'Py (per `project.engineId`).
- **Tier 3 robustness**: no frontend tests exist (add vitest + RTL for store /
  translation queue / UnitRow); add a `translate_units` orchestrator test (tauri
  mock runtime); add a CI **build+test on push/PR** workflow (CI today only does
  release-on-tag in `.github/workflows/release.yml`).
- **README screenshot** of the redesigned UI (repo has a README but no image).
- Optional: a manual **"Check for updates"** button (the updater only checks on
  startup today via `src/components/UpdateBanner.tsx`).

## Files
- New per engine: `src-tauri/src/engine/<name>.rs`,
  `src-tauri/tests/<name>_roundtrip.rs`, `src-tauri/tests/fixtures/<name>-sample/`.
- Edit per engine: `src-tauri/src/engine/mod.rs` (register in `engines()`),
  `src-tauri/src/engine/protect.rs` (`mask_<name>` + `mask_for`),
  `src/codes.ts` (warn regex).
- Backlog touches: `src/messageWidth.ts`, `.github/workflows/`, `README.md`,
  `src/components/UpdateBanner.tsx`.

## Verification (per new engine)
- `cargo test` (or `--lib` + `--test <name>_roundtrip` if the dev app holds the
  exe lock) green, including the new round-trip identity test.
- `cargo run --example harness -- extract <game>` → sensible kind breakdown, 0
  round-trip mismatches; bare code/asset strings NOT extracted.
- `cargo run --example harness -- ai <game> translategemma:12b <n>` → real
  translation with inline codes preserved through mask/restore.
- In-app: open a real game → detect the engine → Run a batch → Preview → Export.
- Release: bump `package.json` + `src-tauri/Cargo.toml` + `Cargo.lock` +
  `src-tauri/tauri.conf.json`, tag `vX.Y.Z`, push tag → CI publishes installers +
  portable + signed `latest.json`.
