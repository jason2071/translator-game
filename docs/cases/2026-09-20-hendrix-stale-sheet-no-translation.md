---
title: "Hendrix — stale CSV exports Thai that the game cannot display"
tags:
  - type/case
  - engine/hendrix
created: 2026-09-20
status: fixed-in-0.19.5
---

# Hendrix — stale CSV exports Thai that the game cannot display

**Game:** Arielle's Descent v0.1. Export added a Thai `th` column and a language
picker, but dialogue stayed English after the player selected Thai.

**User-visible failure** — export reported success and the Thai language choice
appeared, yet in-game dialogue was not translated.

## Root cause

1. Hendrix translates at runtime by exact lookup: every live game string must
   appear in `game_messages.csv`'s `Original` column.
2. The shipped sheet contained text from a different/older build (for example,
   `Road to Dornhollow`), while current maps contained different dialogue (for
   example, `I should go explore...`).
3. The app treated the sheet as authoritative, translated its 4,769 rows, and
   appended `th`; Hendrix therefore loaded a valid Thai column with no keys for
   the live dialogue.

## Game-side fix

Regenerate/update `game_messages.csv` through the running Hendrix game first,
then re-import its rows, translate them, and export again. Preserve the old
sheet as a backup: translations whose original text no longer exists cannot be
reused automatically.

## Repo fix

Before Hendrix export, the app now extracts live RPG Maker dialogue/choices,
groups multiline messages as Hendrix does, and compares them with the sheet's
`Original` keys. For games with at least 20 live messages, export stops when
fewer than 10% match, explaining how to regenerate the sheet. Regression tests
cover both a stale sheet and a matching sheet.

## Rule

Never consider a Hendrix CSV valid solely because it parses or has a language
column. The runtime can only translate exact `Original` keys, so verify that
the table substantially overlaps the current game data before exporting.
