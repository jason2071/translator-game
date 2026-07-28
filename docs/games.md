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
| [[wolf-rpg]] | Wolf RPG Editor (ウディタ) — `.wolf` DXArchives → `.mps`/`.dat` (e.g. 山王寺家の人々) | 🟠 Text is easy once unpacked (command `{code,intArgs,stringArgs}` → JSON Pointer, WolfTL does the binary); 🔴 archive crypt (this game = crypt **331** / v3.31, AES-encrypted header addresses) — left to external UberWolfCli | **removed** — the `wolfrpg` engine was built (dump-based, validated against a real 330-file dump: 10 633 units) and then dropped at the user’s call; the note stays as the format record |

## Backlog ideas (not yet researched)

- Unreal Engine `.locres` — documented binary table. (A UE5 sample on disk,
  *Saida*, ships **no game `.locres`** — only `en/Engine.locres` — so its text is
  inside Blueprints/DataTables in `.ucas`; the harder variant.)

## See also

- [[Home]] — docs map of content
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
