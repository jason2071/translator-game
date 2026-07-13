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
status: implemented
---

# Unity — TextTable MonoBehaviour (Mono + Addressables)

A **fourth, distinct** Unity target next to [[unity-naninovel]] (binary `.assets`
TextAssets) and [[unity-csv-localization]] (plaintext CSV). Here **all** in-game text
lives in a small number of **custom `TextTable` MonoBehaviours** — a per-language
string matrix serialized inside an Addressables bundle. Built and validated end-to-end
on **NTR Soccer** (otomi-games).

Engine id **`unity-textbl`**, name **Unity (TextTable)**
(`src-tauri/src/engine/unity_textbl.rs`), driven by the shared UnityPy helper
`resources/unity/rpgtl_unity.py` (`texttable-export` / `texttable-import` /
`catalog-crc`, plus the existing `swap-font`).

> **Status: implemented.** Rust engine + helper commands + protect/codes mirror +
> env-gated test all in; text / font / CRC all validated technically (PoC below).
> Pending: a real in-game launch to confirm the Default column shows Thai.

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

Plus **more TextTables in the `characterpose` bundle** (H-scene bubbles). Non-empty
across both bundles: **550 fields**. These are the **UI + SFX** tier — **not** the
story dialogue.

> ⚠ **Earlier research wrongly concluded "no narrative."** The story dialogue is NOT in
> the TextTables — it lives in a **PixelCrushers Dialogue System database** (see *Tier 2*
> below). `read_typetree` fails on that MonoBehaviour (stripped typetree) + its script
> class doesn't resolve, so the first scan (by typetree keys / script name) missed it.
> **~2528 dialogue lines** — the real bulk of the game's text.

## Tier 2 — the PixelCrushers Dialogue System database

`sharedassets0.assets` (a plain `.assets`, **not** a bundle) holds **one large
`DialogueDatabase` MonoBehaviour** (pathId 3517, ~4.2 MB, stripped typetree). Its
strings are Unity length-prefixed UTF-8; each `DialogueEntry` field serializes as
`[title][value][CustomFieldType_…]`. The translatable line is the value right after a
**base** `"Dialogue Text"` / `"Menu Text"` title (localized variants carry a locale
suffix — `"Dialogue Text ja"` — and are left alone; the **base holds the shown/English
text**, which we overwrite → Thai). Extracted by **raw-string enumeration + byte
splice** (same machinery as the Naninovel dialogue tier — `enum_strings` /
`splice_string`), pointer `"ds#<file>#<pathId>#<idx>"`. **2528** base lines
(`Dialogue Text` + `Menu Text`), 0 title/marker leaks. Lines carry `[pic=N]` portrait
tags + `<color>` TMPro tags → masked via `mask_unity_textbl` (`mask_unity` + `[…]`).

**Grand total ≈ 3078 translatable strings** (550 TextTable + 2528 Dialogue System) —
the whole game.

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

## Where text is / isn't

- ✅ **UI + SFX** → the `TextTable` MonoBehaviours (tier 1, 550 fields).
- ✅ **Story dialogue** → the **PixelCrushers `DialogueDatabase`** in
  `sharedassets0.assets` (tier 2, ~2528 lines). *(The first scan missed it because
  `read_typetree` fails on the stripped MB and its script class doesn't resolve — find
  it by a raw-byte marker instead: a MonoBehaviour blob containing `b"Dialogue Text"` +
  `b"CustomFieldType"`.)*
- ❌ **PlayMaker FSMs (1216)** carry no narrative — they read DS/DB **variables** and set
  TMP text at runtime (`setTextmeshProUGUIText`) from the TextTable / DS by id.
- ❌ **578 TMP components** hold only placeholder UI labels (`Lock`, `Cam`, `Pose`, …),
  runtime-overwritten by `LocalizeUI` (375). Not a source.

**Lesson:** on a Unity game, don't conclude "no narrative" from a failed typetree /
unresolved script-class scan — **grep the raw MonoBehaviour blobs** for the storage's
string markers (here PixelCrushers' `Dialogue Text` / `CustomFieldType`). A stripped
typetree hides the structure, not the strings.

## Fonts

- **9 `TMP_FontAsset`** in `s.event` bundle + two `LocalizedFonts` MBs (`Localized
  Fonts`, `Soccer Localized Fonts`) that map language → TMP font, with a
  `defaultTextMeshProFont` for the base/`en`/Default column.
- **RESOLVED — both Default fonts are Dynamic-atlas** (`m_AtlasPopulationMode == 1`):
  `PlaypenSans-VariableFont_wght SDF` and `851tegaki_zatsu_normal_0883 SDF`. Dynamic
  mode rasterizes glyphs at runtime from an in-bundle source `Font`, so the existing
  `rpgtl_unity.py` **`swap-font`** (swap the source TTF → bundled Sarabun) makes Thai
  render — **no SDF baking**, same as [[unity-csv-localization]]. (Only
  `NotoSansJP-Regular SDF` is static, mode 0 — the JP column, which we don't target.)
  `embed_font` sweeps `swap-font` over every bundle; a bundle with no dynamic font
  writes no output and is skipped.

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

## Validation (PoC, all via the helper on the real game)

- **`texttable-export`**: **550** non-empty fields, **~5.9 s**. TextTables live in
  **two** bundles, not one — `s.event` (275) **and** `characterpose` (275). The engine
  sweeps every `StandaloneWindows64/*.bundle`, so both are covered.
- **`texttable-import`** (splice `Default` → Thai, repack): **~77.6 s** for the 1 GB
  `s.event` (+ 91 MB `characterpose`); output `s.event` = 1016 MB and **re-loads**;
  each spliced field reads back as the Thai value (**load-faithful**). Risk #5 (1 GB
  repack memory/time) → acceptable.
- **`dsdb-export`**: **2528** dialogue lines from `sharedassets0.assets` (244 MB) in
  **~2.8 s**. **`dsdb-import`** (splice a few → write the `.assets`): **~2.0 s**; output
  re-exports the spliced lines as Thai (**load-faithful**, 0 mismatches). `.assets` are
  plain Unity files (no Addressables CRC to clear — that's only the `aa/` bundles).
- **Font** (see above): both Default fonts are Dynamic → `swap-font` applies.
- **CRC**: catalog is **`catalog.json`**; per-bundle `AssetBundleRequestOptions` are
  **UTF-16LE JSON** inside `m_ExtraDataString` (base64), 4-byte LE length-prefixed:
  `…<i32 len>{"m_Hash":"…","m_Crc":<n>,…}…`. The observed CRCs are **non-zero** (the
  game verifies), so a modified bundle needs them cleared. `catalog-crc` decodes the
  blob, sets each `m_Crc` to 0, fixes the length prefix, re-base64s — validated
  (4 CRCs → 0, hashes intact). *(Contrast [[unity-csv-localization]]: a binary
  `catalog.bin`, CRC at `hash+60`.)*

## Fonts — the dynamic-swap approach does NOT work here; SDF baking does

Unlike [[unity-csv-localization]] (Milf Plaza), NTR's TMP fonts ship a **pre-baked**
glyph/character table + atlas and the runtime does **not** rasterize new glyphs
dynamically (`Player.log`: Thai "not found in [PlaypenSans …] font asset or any potential
fallbacks", even after swapping the source TTF to a Thai font + clearing the atlas). So
`swap-font` alone leaves Thai as tofu. Thai must be **baked into the atlas + tables
offline** (SDF), proven in-game (dialogue renders Thai). Reference tooling +
calibration: `scripts/unity-sdf-bake/` (freetype render → scipy signed-distance →
`alpha = clip(128 + 13·dist, 0, 255)`, edge 128; pack into free atlas space; append
glyph/char entries; set the font Static).

⚠ **The font the subtitle uses is a THIRD copy** —
`PlaypenSans-VariableFont_wght SDF` exists in `s.event`, `characterpose`, **and
`sharedassets0.assets` (pid 3527) with a stripped typetree** — and the stripped one is
what renders dialogue. Editing the two bundle copies changed nothing; the fix is a
**raw-blob transplant** onto pid 3527 (build the 176-char font blob from a bundle copy's
full typetree, fix its PPtrs to `sharedassets0`'s atlas/material/source-font/script,
`set_raw_data`), plus baking the Thai SDF into that file's atlas. Not yet an app feature
(freetype/scipy weight + stripped-transplant generality are the open productization
questions); other UI fonts (`851tegaki…`) still need the same treatment.

## Resolved notes

- **Default = shown column for `en`.** Overwriting `m_values[0]` targets the base
  column; the `en` Locale maps to `Default`. *(Final in-game confirmation still pending
  a launch, but this is the design and matches the Milf Plaza overwrite-a-locale UX.)*
- **Mixed EN/JP base** — the extractor takes `m_values[0]` verbatim (EN or JP); both are
  "the text to translate". The `unity` mask (TMPro tags + `{TOKEN}`/`{0}`) is reused.
- **Export is in-place only.** Bundles are gigabyte-scale, so no mod-staging copy is
  offered (like Ren'Py/Hendrix). Originals are snapshotted under `.rpgtl/source/` on the
  first export (undo + inject-from-original for idempotent re-export).

## Next steps (checklist)

- [x] Confirm font mode + inspect `LocalizedFonts` MB → both Default fonts Dynamic.
- [x] PoC: splice one `Default` → Thai, repack, reload-verify (load-faithful). *(In-game
      launch confirmation still TODO.)*
- [x] CRC handling from `catalog.json` → `catalog-crc` (UTF-16 JSON `m_Crc` → 0).
- [x] Implement `engine/unity_textbl.rs` (detect/extract/inject/embed_font) + helper
      `texttable-export` / `texttable-import` / `catalog-crc` + tolerant `swap-font`.
- [x] `mask_for` branch (reuse `mask_unity`) + `src/codes.ts` / `messageWidth.ts` mirror.
- [x] `tests/unity_textbl_roundtrip.rs` (env-gated real-game: `RPGTL_TEXTBL_GAME` /
      `RPGTL_TEXTBL_WRITE`) + lib unit tests (detect, pointer).
- [x] Row in [[games]] index; `status: implemented`.
- [ ] **Launch NTR Soccer after a real export** to confirm Thai shows + renders.
- [ ] Add rows to [[ENGINES]] + [[ROADMAP]].

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
