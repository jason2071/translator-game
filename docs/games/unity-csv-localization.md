---
title: Unity — CSV localization (IL2CPP + Addressables)
aliases:
  - Unity CSV localization
  - unity-csvloc
  - Milf Plaza
tags:
  - type/research
  - engine/unity-csvloc
  - game/milf-plaza
created: 2026-07-12
status: implemented
---

# Unity — CSV localization (IL2CPP + Addressables)

A **second, unrelated** Unity target next to [[unity-naninovel]]. Where Naninovel
buries strings in binary `.assets`, this family keeps **all** its text in plaintext
CSV catalogs the game reads at runtime — so the text layer is the easiest we handle,
and the real work is fonts. Proven end-to-end **in-game** on **Milf Plaza**
(company *Texic* / *Milfarion*), an **IL2CPP + Addressables** build.

Engine id **`unity-csvloc`**, name **Unity (CSV localization)**
(`src-tauri/src/engine/unity_csv.rs`).

## Storage

```
<name>_Data/StreamingAssets/Localization/
├── english/   ← 11 CSV catalogs (dialogs, ui, characters, items, …) + meta.txt
├── russian/
└── <target>/  ← we write this
```

- Each catalog is `key;value`, **`;`-delimited**, **CRLF**, no header, no BOM.
- Values are **never quoted** and **never contain a `;`**; `""` and `\n` appear
  **literally** and are treated as opaque (so each line splits on its first `;`).
- Each locale folder has a `meta.txt` = `{"_visibleName":"English","_author":""}`.
- The game **folder-scans** `Localization/` and reads each `meta.txt`, so **adding a
  new `<lang>/` folder makes it a selectable in-game language** — no code hook.

Milf Plaza (demo v0.0.7d) ships ~5,735 source strings (dialogs 3,121 /
interactable_scenes 2,220 / ui 197 / …). English already ships, so the app
translates **English → Thai** (like the Naninovel locale slot).

## How the engine works

- **detect** — a `<name>_Data/StreamingAssets/Localization/<lang>/` folder with a
  `meta.txt` + at least one `.csv`. Unique to this scheme, so Naninovel and plain
  Unity games are declined.
- **extract** — parse the source locale (prefer `english/`); one unit per non-empty
  value, `pointer = "start:len"` (the value's byte span, Godot-style), context = key.
- **inject / `export_locale`** — **additive, parallel-locale** (like Ren'Py `tl/`):
  rebuild each source CSV by splicing translations into the value spans and write it
  to a **new `<target>/` locale folder** (source locales untouched) + a `meta.txt`.
  Untranslated catalogs are copied verbatim so the locale is complete. An unchanged
  unit reproduces the original bytes → **true byte-identity round-trip**.

## Fonts (the hard part) + Addressables CRC

The stock TMPro fonts (LiberationSans / berlinsansfb / Onest) have **no Thai
glyphs** → translated Thai renders as "tofu" boxes. But every font's fallback chain
ends at a **Dynamic-atlas** `TMP_FontAsset` (`m_AtlasPopulationMode == 1`) whose
`m_SourceFontFile` is an in-bundle Unity `Font`. Dynamic mode **rasterizes glyphs at
runtime** from that TTF, so:

- **`embed_font`** swaps that Font's bytes for the bundled Sarabun (Thai + Latin) in
  every `fonts*_*.bundle` — via the shared `rpgtl_unity.py` **`swap-font`** command
  (UnityPy; typetree **is** readable here, unlike Naninovel). No SDF atlas baking.
  Re-saved LZ4 (stays ~50 MB, not the 207 MB uncompressed).
- The bundle is **Addressables-CRC-verified**, so a modified bundle otherwise fails
  with `CRC Mismatch … Will not load AssetBundle` and **hangs at the loading
  screen**. So `embed_font` also **zeroes the bundle's Crc in `catalog.bin`** — a
  pure-Rust byte patch: take the 32-hex hash from the bundle filename → 16 raw bytes
  → find in `catalog.bin` (unique) → the `Crc u32` sits at **hash offset + 60** →
  write `0` (Addressables then skips verification). Catalog layout per bundle:
  `[len][filename][len][internal-id][16-byte raw hash][len=32][md5 str][u32][u32][Crc u32]`.
  Works because the catalog is loaded from `catalog.bin` on disk
  (`m_IsLocalCatalogInBundle=false`, no remote catalog).

## Validation

- **Language slot** — a `thai/` folder appeared as a selectable language (folder
  scan) ✅.
- **Text** — sample Thai loaded into the right catalogs ✅.
- **Font** — after the dynamic-fallback TTF swap + CRC-zero, Thai renders correctly
  in the menu (tone/vowel marks stacked right), no tofu ✅.
- **CRC** — before the patch, the modified bundle hung at load with the CRC-mismatch
  log; after zeroing, it loads ✅.

## Known gaps / notes

- `embed_font` needs the UnityPy sidecar (frozen exe in release, else system Python)
  — same infra as [[unity-naninovel]]; the CI test suite covers the CSV logic + the
  CRC patch, not the binary bundle edit.
- Speaker context: the per-locale CSVs carry only `key;value`; a richer speaker hint
  (from the game's `all_localization.csv` `говорящий` column) is a future
  enhancement.
- Windows-only bundle path (`StandaloneWindows64`) for now.

## See also

- [[unity-naninovel]] — the other Unity engine (binary `.assets`)
- [[games]] — per-game research index · [[ENGINES]] · [[ROADMAP]]
