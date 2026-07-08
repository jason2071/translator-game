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
| [[anvilnext-forger]] | AnvilNext — AC Origins / Odyssey / Valhalla (`.acod` via Forger) | 🟢 Easy (text layer) + external Forger/FontForge | **planned** — phased plan + format confirmed; blocker = EN source `.acod` from Forger |

## Backlog ideas (not yet researched)

- Unity (I2Localization CSV / TextMeshPro) — big indie audience; font via TMP asset.
- Unreal Engine `.locres` — documented binary table.
- Wolf RPG (`Data.wolf`) — often encrypted; needs a decryptor first.

## See also

- [[Home]] — docs map of content
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
