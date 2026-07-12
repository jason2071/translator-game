---
title: Unity — TextTable MonoBehaviour (Mono + Addressables)
aliases:
  - Unity TextTable
  - unity-textbl
  - NTR Soccer
tags:
  - type/research
  - engine/unity-textbl
  - game/ntr-soccer
created: 2026-07-13
status: proposed
---

# Unity — TextTable MonoBehaviour (Mono + Addressables)

A **fourth, distinct** Unity target next to [[unity-naninovel]] (binary `.assets`
TextAssets) and [[unity-csv-localization]] (plaintext CSV). Here **all** in-game text
lives in a small number of **custom `TextTable` MonoBehaviours** — a per-language
string matrix serialized inside an Addressables bundle. Investigated (not yet built)
on **NTR Soccer** (otomi-games).

Proposed engine id **`unity-textbl`**, name **Unity (TextTable)**
(`src-tauri/src/engine/unity_textbl.rs`).

> **Status: research complete, engine not started.** Continue from *Next steps*.

## Game facts (NTR Soccer)

- Path (dev machine): `F:\Downloads\otomi-games.com_RKI5ZOR34\NTR Soccer`
- **Unity Mono backend** (`NTR_Soccer_Data/Managed/*.dll` present, incl.
  `Unity.Localization.dll`, `Unity.TextMeshPro.dll`) → **typetrees are readable AND
  writable** by UnityPy. *This is the key difference from Milf Plaza (IL2CPP).*
- **Addressables** with a **`catalog.json`** (not `catalog.bin`) under
  `NTR_Soccer_Data/StreamingAssets/aa/`. Four bundles in `StandaloneWindows64/`:
  - `s.event_assets_all_*.bundle` (~1 GB) — **holds the 2 TextTables** + all cutscene
    art/audio/prefabs (26 789 objects; mostly Sprite/Texture2D).
  - `characterpose_assets_all_*.bundle` (~91 MB) — character art.
  - `localization-locales_assets_all.bundle` (~3 KB) — **5 `Locale` objects only**.
  - `unitybuiltinshaders_*.bundle`.
- **5 Locales**: `ja`, `en`, `ko`, `zh`, `zh-TW` (English already ships → translate
  **EN→TH**, like the other Unity engines).
- Also uses **PixelCrushers Dialogue System** and **PlayMaker** — but see
  *What is NOT there*: neither stores narrative text.

## Storage — the `TextTable` MonoBehaviour (decoded)

Two instances live in `s.event_...bundle` (script class `TextTable`):

| m_Name | fields | content |
|--------|-------:|---------|
| `BubbleText Text Table` | 56 | H-scene bubble SFX / short moans (`Ah…`, `Mm…`, …) |
| `UI Localization Text Table` | 228 | menus, buttons, days/times, tutorials, minigame instructions, **character bios**, place/item names, more moans |

**≈ 284 strings total — that is essentially the whole game's text.** No long branching
VN narrative exists separately (see below).

Typetree shape (both tables identical):

```
MonoBehaviour (script "TextTable")
  m_Name          : "UI Localization Text Table"
  m_languageKeys  : ['Default','ja','zh','zh-tw','ko']   # Default = base column
  m_languageValues: [int, ...]      # not text — parallel bookkeeping ints
  m_fieldKeys     : [1,2,3,...]     # int row ids
  m_fieldValues   : [ Field, ... ]  # <-- the text lives here
  m_nextLanguageID, m_nextFieldID : int
```

Each `Field` in `m_fieldValues`:

```json
{
  "m_fieldName": "Ah…",            // usually == the base string (a label/key)
  "m_keys":   [0, 1, 2, 3, 4],      // language indices, parallel to m_languageKeys
  "m_values": ["Ah…", "あ…", "啊…", "啊…", "아…"]   // one string per language
}
```

- `m_values[0]` = the **`Default`** column = the **base language shown when the `en`
  Locale is selected**. This is what we translate.
- Alignment: `m_values[k]` ↔ `m_languageKeys[k]` ↔ `m_keys[k]`. Index **0 = Default**,
  1 = ja, 2 = zh, 3 = zh-tw, 4 = ko.

### ⚠ Mixed base language

Most `Default` values are **English**, but **some are Japanese** (character bios at
UI indices ~20–23, a few UI labels like `ボイス`/`言語`/`購入`/`ロード`). So the extractor
must treat the source as **EN-or-JP** (both are "the text to translate"), not assume
English. Empty strings (`''`) appear too — skip them.

## What is NOT there (checked, so don't re-hunt tomorrow)

- **No PixelCrushers DialogueDatabase asset.** Scanned all 4 bundles + `resources.assets`
  + every `sharedassets*.assets` + `level*` + `globalgamemanagers*` for MonoBehaviours
  whose script name contains Dialogue/Database/Conversation, and by typetree keys
  (`conversations`/`actors`/`dialogueEntries`) — **zero**. DS is used **only for
  variables** (affection counters / flags), via `PixelCrushers.DialogueSystem.PlayMaker.GetVariable`.
- **No large `TextAsset`** anywhere (no chat-mapper CSV/JSON > 4 KB).
- **PlayMaker FSMs (1216)** carry **no narrative** — a JP-string(≥8) walk = 0 hits, a
  long-string(≥20) walk returns only action **type names** (`HutongGames.PlayMaker.Actions.*`).
  FSMs are game logic; they set TMP text at runtime (`setTextmeshProUGUIText`) pulling
  from the TextTable by field id.
- **578 TMP components** with text hold only **placeholder UI labels** (`Lock`, `Cam`,
  `Auto`, `Pose`, `2x`, …), runtime-overwritten by `LocalizeUI` (375 of them) from the
  TextTable. Not a translation source.

**Conclusion:** NTR Soccer is a **gameplay-driven** H-game (soccer minigames +
pose-viewer H-scenes with short bubble moans). Translating the 2 TextTables = the
whole game. Low text volume, so the value is the **engine**, not the word count.

## Fonts

- **9 `TMP_FontAsset`** in `s.event` bundle (+ a `LocalizedFonts` MonoBehaviour ×2 that
  likely swaps font per locale). Stock fonts have no Thai → need a Sarabun swap, same
  problem class as [[unity-csv-localization]].
- **Open:** are these TMP assets **dynamic-atlas** (`m_AtlasPopulationMode == 1`, swap
  source `Font` TTF like Milf Plaza) or **static SDF atlas** (must bake glyphs)? Must
  check tomorrow. If dynamic, reuse the `rpgtl_unity.py` **`swap-font`** path. The
  `LocalizedFonts` MB may let us point the `en`/Default locale at a Thai-capable font
  without touching every asset — inspect it.

## Proposed engine design

Mirror the [[unity-naninovel]] engine (bundled UnityPy helper, load-faithful
round-trip) rather than the byte-span engines — this is typetree read+**write**.

- **detect** — a `<name>_Data/` with `StreamingAssets/aa/catalog.json` **and** a bundle
  containing a MonoBehaviour whose script class is `TextTable` with the
  `m_languageKeys`/`m_fieldValues` shape. Decline plain Unity / Naninovel / csvloc.
  (Cheap pre-filter: `Managed/Unity.Localization.dll` + `aa/` dir, then confirm via the
  helper.)
- **extract** — via a new `rpgtl_unity.py` subcommand (e.g. `texttable-dump`):
  enumerate every `TextTable`, for each non-empty `m_values[0]` emit a unit.
  - **pointer** = `"tbl#<bundleFile>#<pathId>#<fieldIndex>"` (idx into the deterministic
    `m_fieldValues` enumeration; same discipline as Naninovel's `dlg#…#<idx>`).
  - **context** = `m_fieldName` (the label) — helps the translator.
  - source text = `m_values[0]` (EN or JP).
- **inject / export** — helper `texttable-splice`: for each unit, set `m_values[0]` (the
  Default column) to the translation, re-serialize the `SerializedFile`, write the
  bundle. **Round-trip = load-faithful** (UnityPy re-serializes the whole file, like
  Naninovel / KiriKiri-UTF16 exception — relax `roundtrip_identity` accordingly).
  - **Thai-by-default**: overwriting the **Default** column means the game shows Thai
    whenever the `en` Locale is active (the default). No in-game language switch, no new
    Locale needed → same UX as the Milf Plaza mod. *(Alternative if Default proves not
    to be the shown column: append a `th` entry to `m_languageKeys`/`m_values` + register
    a `th` Locale in `localization-locales` bundle — more work; try Default-overwrite
    first.)*
- **embed_font** — `swap-font` the (dynamic) TMP source Font → bundled Sarabun
  (`engine::TARGET_FONT`), then handle Addressables integrity:
  - **CRC**: catalog is **`catalog.json`** (text) not `.bin`. Check whether it stores a
    per-bundle `m_Crc` / `Hash` we must zero (like the csvloc `+60` byte patch). JSON
    should be easier — locate the bundle's entry and set its CRC field to 0, or confirm
    the JSON catalog doesn't gate on CRC.
- **protect** — add `mask_unity_textbl` (or reuse `mask_unity`) + a `mask_for` branch.
  Inline codes: TMP rich-text tags `{...}` and DS/PlayMaker tokens like
  `{PLAYER_WINS}`, `{OPPONENT_NAME}` (seen in UI fields 167–172) — **must be masked so
  AI never translates them**. Mirror in `src/codes.ts` / `src/messageWidth.ts`.

## Open questions / risks (resolve tomorrow)

1. **Does overwriting `Default` actually show in-game?** Verify the `en` Locale maps to
   the `Default` column at runtime (the `Locale`↔`languageKeys` mapping: keys are
   `Default/ja/zh/zh-tw/ko` but Locales are `en/ja/ko/zh/zh-TW` — `en` presumably ⇒
   `Default`, `zh-TW` ⇒ `zh-tw`). Confirm by a test export + launch.
2. **Font mode** (dynamic vs static SDF) — decides swap vs bake. Inspect `LocalizedFonts`.
3. **CRC in `catalog.json`** — needed or not?
4. **Mixed EN/JP base** — fine for extraction (both are source), but the AI prompt
   should be told source may be either.
5. **Writing a 1 GB bundle** — UnityPy re-serializes the whole `s.event` bundle (all the
   art/audio too). Confirm memory/time are acceptable and the rewritten bundle still
   loads. (Naninovel already does full-file re-serialize, but on much smaller files.)

## Next steps (checklist)

- [ ] Confirm font mode + inspect `LocalizedFonts` MB (dynamic ⇒ reuse `swap-font`).
- [ ] PoC: `texttable-splice` one field's `Default` value → Thai, repack `s.event`
      bundle, launch NTR Soccer in `en`, verify the string shows Thai (+ renders with a
      Thai font). This proves the whole chain before writing the engine.
- [ ] Decide CRC handling from `catalog.json` inspection.
- [ ] Implement `engine/unity_textbl.rs` (detect/extract/inject/embed_font) + helper
      subcommands in `resources/unity/rpgtl_unity.py`.
- [ ] `mask_for` branch + `src/codes.ts` mirror (mask `{TOKENS}` + TMP tags).
- [ ] Fixture + `tests/unity_textbl_roundtrip.rs` (env-gated real-game like csvloc).
- [ ] Row in [[games]] index + [[ENGINES]] + [[ROADMAP]]; flip `status:` to implemented.

## Appendix — probe scripts

All run against `…\NTR Soccer\NTR_Soccer_Data\StreamingAssets\aa\StandaloneWindows64`
with system Python + `UnityPy 1.25.2`. Console is cp874 (Thai) → **write results to a
UTF-8 file**, don't print JP/TH to stdout.

- Read the two tables (schema + sample):
  ```python
  import UnityPy, json
  env = UnityPy.load(r"...\s.event_assets_all_*.bundle")
  for o in env.objects:
      if o.type.name=="MonoBehaviour":
          t=o.read_typetree()
          if t.get("m_fieldValues") and "m_languageKeys" in t:
              # t["m_languageKeys"], t["m_fieldValues"][i] = {m_fieldName,m_keys,m_values}
              ...
  ```
- The two table `path_id`s observed: `BubbleText` = `-7153854280235068207`,
  `UI Localization` = `-4473351832413774421` (may differ if the game is repacked; match
  by the `TextTable` script + `m_Name` instead of hardcoding).
- Locales: `UnityPy.load(...localization-locales...)` → 5 `Locale` MBs, read
  `m_Identifier.m_Code` + `m_LocaleName`.

## See also

- [[unity-naninovel]] — sibling Unity engine (typetree read+write, load-faithful) — the
  closest template for this one.
- [[unity-csv-localization]] — sibling Unity engine; source of the **font-swap** +
  **Addressables CRC** patterns.
- [[games]] — research index · [[ROADMAP]] · [[ENGINES]]
