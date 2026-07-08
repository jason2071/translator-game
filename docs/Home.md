---
title: Docs — Home
aliases:
  - Home
  - MOC
  - Docs
tags:
  - moc
created: 2026-07-08
---

# 📖 Docs — Home

Map of content for the `docs/` vault. This folder is both the repo's documentation
and an Obsidian vault; start here.

## Core references

- [[ENGINES]] — game engine landscape + translatability feasibility (what text
  lives where, and how well it fits our extract → inject + round-trip model).
- [[ROADMAP]] — next engines, ranked alternatives, backlog, and the reusable
  **engine-adding pattern**.
- [[QA-TEST-PLAN]] — manual + automated QA test plan.

## Research

- [[games]] — game-translation research index (per-game / per-engine deep dives).
  - [[anvilnext-forger]] — Assassin's Creed (Origins/Odyssey/Valhalla) via Forger `.acod`; research + phased implementation plan for a `forger_acod` engine.

## Conventions (Obsidian best practice)

- **Folders by topic.** Deep-dive research lives under `docs/games/`; each topic
  folder gets a **folder note** of the same name (`games.md`) acting as its index.
- **Frontmatter** on every note — `title`, `aliases`, `tags`, `created`, `status`.
- **Wikilinks** (`[[note]]`) between notes, not raw paths, so the graph view and
  backlinks work.
- **Tag taxonomy** — nested tags: `type/research`, `engine/<name>`,
  `game/<name>`, `moc`.
- **Stable core docs.** `ENGINES.md`, `ROADMAP.md`, `QA-TEST-PLAN.md` keep their
  names/paths — they're referenced from `CLAUDE.md` and `README.md`; don't move
  them.
- **Vault config** (`.obsidian/`) is per-user and git-ignored — not shared state.
