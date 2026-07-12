---
title: Unity — Naninovel managed text
aliases:
  - Unity Naninovel
  - unity engine
tags:
  - type/research
  - engine/unity
  - game/my-milf-stepmom
created: 2026-07-12
status: implemented
---

# Unity — Naninovel managed text

Research + engine note for the `unity` engine: Unity games whose translatable text
is stored as **Naninovel managed-text documents**. Part of [[games]]; see [[ENGINES]]
and [[ROADMAP]].

## The feasibility split (why Unity is per-game, not one answer)

Two real games bracket the range:

- **My MILF Stepmom** — Unity **Mono** + **Naninovel**. **Two** text layers:
  (1) **managed-text docs** in `resources.assets` (~187 usable strings/locale after
  skipping the `Locales` language-name list: UI, character names, gallery
  titles/descriptions, scripted-UI text) — the built-in **`TextAsset` class**, no game
  DLL / typetree needed 🟢 **feasible**; and (2) the **story dialogue**, compiled into
  `Naninovel.Script` MonoBehaviours with **stripped typetrees** (Chinese, one per
  scene) 🔴 **out of reach** (see *Known gaps*). The game ships no script-localization
  docs, so even English mode shows Chinese story — it was never story-localized. Ships
  three managed-text locale copies (zh source + `en` + `zh-HK`).
- **MR 6.2 / Milf's Resort** — Unity **Mono**, custom dialogue, **6 893
  MonoBehaviours, 0 typetree-readable** (typetrees stripped). Text is locked in
  custom serialization needing type info reconstructed from `Assembly-CSharp.dll`.
  🔴 **out of scope** (XUnity.AutoTranslator's runtime-hook territory, not this app's
  file-rewrite model).

So the `unity` engine covers **the `TextAsset`/Naninovel-managed-text case only** and
declines the rest at detection (see below).

## How it works

Unity `.assets` are binary, so — like the AnvilNext engines feed on an external tool
— the binary work is delegated. Here the tool is a **UnityPy helper**
(`src-tauri/resources/unity/rpgtl_unity.py`), driven from `engine/unity.rs` the way
[[../CLAUDE|Ren'Py]] drives the vendored unrpyc decompiler. UnityPy reads/edits a
`TextAsset.m_Script` and re-serializes the `SerializedFile`.

Unity games ship no Python, so the release build **embeds a frozen interpreter**:
`scripts/freeze-unity-sidecar.ps1` runs PyInstaller to build `rpgtl-unity.exe` (a
one-file exe, ~66 MB; UnityPy's texture deps are excluded and stubbed since only
`TextAsset` is touched), `build.rs` `include_bytes!`s it into the Rust binary, and the
engine materializes + runs it — no system dependency. The exe is a git-ignored build
artifact (regenerate before a release build); when it is absent (a plain `cargo
build`, CI, or a non-Windows host) the engine falls back to the **system `python`** +
the plain script and degrades with an actionable "install UnityPy" error.

- **Detection** — a `<name>_Data/` dir with `resources.assets` **and** a Naninovel
  runtime assembly (`*Naninovel*.dll`) in `Managed/`. Plain Unity games lack the
  assembly and are declined (fall through to "no engine detected").
- **Pointer** — engine-opaque `"<file>#<pathId>#<key>"`: a `TextAsset` (by Unity
  path-id) + the managed-text record key. Addresses records *logically*, so re-export
  is stable (the export snapshot restores the original `.assets`; the same
  `pathId#key` still resolves — no byte-offset staleness).
- **Locale slot** — Phase 1 targets the **`en`** localization docs: the player picks
  English in-game and sees the translation; other locales stay intact. A game with no
  such docs falls back to its source docs (base language becomes the translation).
- **Round-trip** — **load-faithful, not byte-identical** (a documented exception like
  KiriKiri's UTF-16 fallback): UnityPy re-serializes the whole file, so an edited
  `.assets` is only structurally equivalent with the patched strings changed. The
  helper emits **only the files it changed**; untouched `.assets` are never
  re-serialized. `inject` runs the helper into a private temp dir then relocates the
  changed files into `out_dir`, because UnityPy holds the source open while saving
  (writing back into the same dir is a Windows sharing violation).
- **Masking** — `protect::mask_unity`: TMPro rich-text tags (`<color=…>`, `<b>`,
  `<br>`), `{0}`/`{name}` format args, and `\n`. `[…]` and `%` stay visible
  (decorative/prose). Mirrored in `src/codes.ts` + `src/messageWidth.ts`.

## Validation

Phase-0 PoC + Phase-1 engine both green on **My MILF Stepmom**:

- export→patch→import round-trip: untouched files byte-identical, target doc patched,
  all other records intact, object count 10 482 = 10 482, only intended bytes changed,
  ~2.7 s for the 251 MB `resources.assets`.
- **In-game confirmed**: patched the `en` docs with a `[TH]` marker → launched the
  game → picked English → `[TH]NEW GAME / CONTINUE / SETTINGS / EXIT / Language` all
  showed.
- Rust engine end-to-end (`tests/unity_roundtrip.rs`, opt-in via `RPGTL_UNITY_GAME`):
  extract → mark every unit → inject → re-extract → marker survives.

## Known gaps / next

- **Story dialogue is out of reach.** In a compiled-script Naninovel game (like
  Stepmom) the dialogue lives in `Naninovel.Script` MonoBehaviours whose typetrees are
  **stripped**. It *is* reachable via typetree-from-DLL (TypeTreeGeneratorAPI loads the
  Managed DLLs, e.g. `get_nodes("…Naninovel.Runtime","Naninovel.Script")` → 31 nodes),
  but the script lines are `[SerializeReference]` polymorphic subclasses
  (`CommandScriptLine` / `GenericTextScriptLine` / `CommentScriptLine`), and UnityPy
  1.25.2 + generator 0.0.10 can't resolve the per-line ref types ("Failed to get ref
  type node"). So the engine translates **UI / managed text only** — a script-heavy
  game gets its menus / names / gallery, not its story; a Naninovel game localized the
  *proper* way (managed script-localization docs) is fully covered. The detect warning
  says so. A fix needs a different toolchain (AssetsTools.NET + Cpp2IL, or a
  ref-type-capable generator) — large and fragile per Naninovel version.
- **Thai glyphs** — Naninovel renders via TMPro; the stock font likely lacks Thai →
  tofu. Injecting a TMP SDF font with Thai glyphs is materially harder than the
  Ren'Py/RPGMaker font swap. Deferred; prove the pipeline with a font-safe marker
  first.
- **Tier 2** — generic `TextAsset` text (plain `.txt`/`.csv`/`.json`, I2 Localization
  CSV-in-TextAsset) with content heuristics.

## See also

- [[games]] — game-research index
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
- [[anvilnext-locpackage-format]] — the other "engine fed by an external tool" pattern
