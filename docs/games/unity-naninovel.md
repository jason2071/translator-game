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

- **My MILF Stepmom** — Unity **Mono** + **Naninovel**. Its *entire* translatable
  surface is 29 managed-text docs in `resources.assets` (~419 strings/locale: UI,
  character names, gallery titles/descriptions, scripted-UI text). Ships three
  locale copies (zh source + `en` + `zh-HK`). Verified there is **no hidden `.nani`
  dialogue** — zero CJK anywhere outside the managed text. Text is in the **built-in
  `TextAsset` class**, which needs no game DLL / typetree to read. 🟢 **feasible.**
- **MR 6.2 / Milf's Resort** — Unity **Mono**, custom dialogue, **6 893
  MonoBehaviours, 0 typetree-readable** (typetrees stripped). Text is locked in
  custom serialization needing type info reconstructed from `Assembly-CSharp.dll`.
  🔴 **out of scope** (XUnity.AutoTranslator's runtime-hook territory, not this app's
  file-rewrite model).

So the `unity` engine covers **the `TextAsset`/Naninovel-managed-text case only** and
declines the rest at detection (see below).

## How it works

Unity `.assets` are binary, so — like the AnvilNext engines feed on an external tool
— the binary work is delegated. Here the tool is a **bundled Python + UnityPy helper**
(`src-tauri/resources/unity/rpgtl_unity.py`), driven from `engine/unity.rs` the way
[[../CLAUDE|Ren'Py]] drives the vendored unrpyc decompiler. UnityPy reads/edits a
`TextAsset.m_Script` and re-serializes the `SerializedFile`.

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

- **Thai glyphs** — Naninovel renders via TMPro; the stock font likely lacks Thai →
  tofu. Injecting a TMP SDF font with Thai glyphs is materially harder than the
  Ren'Py/RPGMaker font swap. Deferred; prove the pipeline with a font-safe marker
  first.
- **Shipping** — Phase 1 uses **system Python + UnityPy** (`pip install UnityPy`);
  degrades with an actionable error when missing. Phase 2 bundles a frozen helper exe
  (Tauri `externalBin`) so there's no system dependency.
- **Tier 2** — generic `TextAsset` text (plain `.txt`/`.csv`/`.json`, I2 Localization
  CSV-in-TextAsset) with content heuristics.

## See also

- [[games]] — game-research index
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
- [[anvilnext-locpackage-format]] — the other "engine fed by an external tool" pattern
