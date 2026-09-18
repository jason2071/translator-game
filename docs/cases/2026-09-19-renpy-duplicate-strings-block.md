---
title: "Ren'Py — duplicate `old` key across two tl strings blocks refuses to boot"
tags:
  - type/case
  - engine/renpy
created: 2026-09-19
status: game-side fix applied 2026-09-19 — no repo fix needed (interim hand file collided with the pipeline's own output)
---

# Ren'Py — duplicate `old` key across two tl strings blocks refuses to boot

**Game:** City Lights & Love Bites — Season 1 (`-th` working copy, Ren'Py
8.5.2, Thai export). Follow-up to
[[2026-09-18-renpy-fontgroup-tofu]]'s extraction fix.

**User-visible failure** — game refused to boot after re-exporting:

```
File "game/tl/thai/zzzz_rpgtl_replay_menus.rpy", line 9, in script
    old "第一次进课堂"
Exception: A translation for "第一次进课堂" already exists at game/tl/thai/rpgtl_menus.rpy:64.
```

## Root cause

While the app-side menu-caption pipeline was still being built, the replay
menus had been translated with a **hand-written interim file**
(`tl/thai/zzzz_rpgtl_replay_menus.rpy`, documented in the case above). The
repo fix then shipped and the user re-imported → translated the 40 captions in
the app → exported, which wrote the pipeline's own
`tl/thai/rpgtl_menus.rpy`. The interim file was still sitting in `tl/thai/`,
so the same `old "第一次进课堂"` key was declared twice — and Ren'Py treats a
duplicate string translation as a hard error (`renpy/translation/__init__.py`
`add()` raises), unlike duplicate say blocks where the later one wins.

## Game-side fix (applied 2026-09-19 ~00:10)

Delete the superseded interim file and its compiled companion:

```bat
del "<game>\game\tl\thai\zzzz_rpgtl_replay_menus.rpy"
del "<game>\game\tl\thai\zzzz_rpgtl_replay_menus.rpyc"
```

Verified: boots to the game ("Loading script took 2117 ms"), `errors.txt` not
rewritten. `rpgtl_menus.rpy` (the pipeline's file) carries all 21 captions.

## Repo fix

None needed — the collision was between a hand-authored artifact and the
pipeline, not between two pipeline outputs. The export already owns exactly
one strings file (`rpgtl_menus.rpy`, rewritten per export).

Wishlist (not scheduled): export could warn when a *foreign* strings block in
`tl/<lang>/` declares an `old` the pipeline is about to write — the boot error
names the file and line, but catching it in-app would save a round-trip.

## Rule

**Violated [[GAME-SAFETY]] rule 9 (documented exceptions only) in spirit: an
interim, hand-authored tl file is a loan, not a fixture.** The moment the
pipeline learns to produce the same strings, the loan must be collected —
delete the hand file *before* the first export that includes the new unit
kind. Ren'Py string translations are global by `old` key: two files declaring
the same key is a boot-fatal duplicate, even when both carry the *same*
translation. When supplementing a game by hand, name the file so it is
obviously ours and track it in `overlay.txt` — that tracking is what makes the
cleanup a one-liner instead of an archaeology dig.
