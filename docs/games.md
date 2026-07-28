---
title: Game-translation research
aliases:
  - Games
  - Game research
tags:
  - moc
  - type/research
created: 2026-07-08
---

# Game-translation research

Folder note (index) for per-game / per-engine translation research — the notes in
`docs/games/`. These are investigations into how a specific game or engine stores
its text and whether it fits this app's extract → translate → inject model. When a
note graduates into an implemented engine it also gets a row in [[ENGINES]] and a
section in [[ROADMAP]].

## Notes

| Note | Engine / games | Feasibility | Status |
|------|----------------|-------------|--------|
| [[anvilnext-forger]] | AnvilNext — AC Origins / Odyssey / Valhalla (`.acod` via Forger) | 🟢 Easy (text layer) + external Forger/FontForge | **implemented** (branch `engine-forger-acod`) — engine + protect + tests green; pending real EN `.acod` validation |
| [[anvilnext-locpackage-format]] | AC Origins `.Localization_Package` → `aclocexport` text | 🟢 Easy (community `aclocexport`/`aclocimport` do the binary; app owns a UTF-8 `Id:`/text format) | **implemented** — `ac-loctext` engine (branch `engine-forger-acod`); format confirmed on 33 787 real Origins records; tests green. Supersedes the binary-RE idea |
| [[unity-naninovel]] | Unity (Mono) — Naninovel managed-text `TextAsset`s (e.g. My MILF Stepmom) | 🟢 Easy (built-in `TextAsset`, no typetree) via bundled UnityPy helper; 🔴 stripped-typetree custom Unity games declined | **implemented** (Phase 1) — `unity` engine + `mask_unity` + tests green; validated in-game. Ships behind system Python until the frozen-helper bundle (Phase 2) |
| [[unity-csv-localization]] | Unity (IL2CPP + Addressables) — plaintext `StreamingAssets/Localization/<lang>/*.csv` (e.g. Milf Plaza) | 🟢 Easy text (plaintext CSV, parallel-locale export); 🟠 fonts via dynamic-fallback TTF swap + Addressables CRC-zero | **implemented** — `unity-csvloc` engine + `swap-font` sidecar cmd + CRC patch; text/font/CRC all validated in-game |
| [[unity-texttable]] | Unity (**Mono** + Addressables) — custom `TextTable` MonoBehaviour string matrix (e.g. NTR Soccer) | 🟢 Text (Mono typetree read+**write**, 550 fields across 2 bundles, translate `Default` column); 🟠 fonts TMP dynamic-swap + `catalog.json` CRC (UTF-16 JSON `m_Crc`→0) | **implemented** — `unity-textbl` engine + helper `texttable-*`/`catalog-crc`; text/font/CRC validated (PoC). Pending in-game launch |
| [[wolf-rpg]] | Wolf RPG Editor (ウディタ) — `.wolf` DXArchives → `.mps`/`.dat` (e.g. 山王寺家の人々) | 🟠 Text is easy once unpacked (command `{code,intArgs,stringArgs}` → JSON Pointer, WolfTL does the binary); 🔴 archive crypt (this game = crypt **331** / v3.31, AES-encrypted header addresses) — left to external UberWolfCli | **implemented** (option A) — `wolfrpg` engine reads/writes a WolfTL dump; fixture tests green. Pending a real dump + in-game check; font hook deferred |

## Backlog ideas (not yet researched)

- Unity I2Localization CSV / generic `TextAsset` text — Tier 2 of [[unity-naninovel]].
- Unreal Engine `.locres` — documented binary table. (A UE5 sample on disk,
  *Saida*, ships **no game `.locres`** — only `en/Engine.locres` — so its text is
  inside Blueprints/DataTables in `.ucas`; the harder variant.)

## See also

- [[Home]] — docs map of content
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
