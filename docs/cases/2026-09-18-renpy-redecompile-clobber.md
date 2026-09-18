---
title: "Ren'Py — re-decompile clobbers every .rpy and second-generation output is broken"
tags:
  - type/case
  - engine/renpy
created: 2026-09-18
status: repo fix implemented 2026-09-18 (unreleased) — targeted decompile + fail-safe deletion, regression tests in `renpy_roundtrip.rs`
---

# Ren'Py — re-decompile clobbers every .rpy and second-generation output is broken

**Game:** City Lights & Love Bites — Season 1 (compiled-only Ren'Py 8.5.2, Thai
export via `tl/thai/`). Same game as
[[2026-09-18-renpy-empty-screen-decompile]] — the same afternoon, a second,
larger breakage.

**User-visible failure** — game refused to boot after re-opening the project and
re-exporting (15:33):

```
File "game/ds_values/character_screens.rpy", line 88: Line is indented, but the preceding one-line python statement does not expect a block. Please check this line's indentation. You may have forgotten a colon (:).
    current_hour = tm.hour

File "game/nqtr_screens/screens_nqtr_component.rpy", line 238: Line is indented, but the preceding one-line python statement does not expect a block.
    my_action = [

File "game/nqtr_screens/screens_nqtr_component.rpy", line 400: 'room_action' is not a keyword argument or valid child of the screen statement.
    room_action = [

File "game/nqtr_screens/screens_nqtr_component.rpy", line 501: 'room_action' is not a keyword argument or valid child of the screen statement.
    room_action = [

File "game/phone/apps/applications.rpy", line 169: Line is indented, but the preceding one-line python statement does not expect a block.
    max_page = min(

File "game/phone/apps/calendar/routine_calendar.rpy", line 880: 'girl_only_source' is not a keyword argument or valid child of the screen statement.
    girl_only_source = [
```

## Root cause

A chain with two of our bugs and one Ren'Py behavior:

1. **The game recompiles our decompiled source.** When it boots with a
   decompiled `.rpy` newer than its `.rpyc`, Ren'Py recompiles and rewrites the
   `.rpyc` (mtimes moved to the boot time). The `.rpyc` is now
   *second-generation* — legal bytecode, but structured differently from what
   the developer shipped.
2. **`run_unrpyc` passes `-c` = `--clobber`.** unrpyc rewrites the `.rpy` next
   to *every* `.rpyc` in the game dir, not just the ones that were missing.
   Anything that makes `needs_decompile` true — e.g. deleting one broken
   `.rpy` by hand, as the previous case's repair did — re-decompiles the whole
   game through the clobber.
3. **unrpyc renders multi-line python-in-screen from second-generation bytecode
   as invalid script.** The PyCode carries multi-line source; unrpyc emits a
   bare `$` (empty one-line python) followed by the code at broken indentation.
   Depending on what follows, Ren'Py reports "one-line python statement does
   not expect a block" or "'x' is not a keyword argument or valid child of the
   screen statement". unrpyc still exits 0 reporting "successfully decompiled"
   (reproduced in a sandbox: 4 files → exactly the 6 empty-`$` sites).

A silent variant is possible: an empty-`$` artifact followed by a line that
parses as a valid screen child would *lose code without a parse error*. A
full-game scan found the signature in only the 4 known files this time.

## Game-side fix (applied 2026-09-18 ~15:50)

Delete the four broken `.rpy`; the game loads its own `.rpyc`:

```bat
del "<game>\game\ds_values\character_screens.rpy" ^
    "<game>\game\nqtr_screens\screens_nqtr_component.rpy" ^
    "<game>\game\phone\apps\applications.rpy" ^
    "<game>\game\phone\apps\calendar\routine_calendar.rpy"
```

Verified: boots to the main menu, `log.txt` "Loading script took 5511 ms",
`errors.txt` not rewritten. Scanned every other decompiled `.rpy` for the
`^\s*\$\s*$` signature — no other file carries it.

**⚠ Do not re-open this project in a build older than the repo fix below** —
the four deleted `.rpy` leave `needs_decompile` true, and a pre-fix build will
clobber-and-break the game again on next open. With the fix, re-opening
decompiles only those four (+ the stub screen), auto-deletes the four broken
renders, and the game stays bootable.

## Repo fix (implemented 2026-09-18, unreleased)

1. **Stop the clobber.** `ensure_decompiled` now decompiles only what is
   missing: `missing_sources(dir)` collects the `.rpyc`/`.rpymc` without a
   source sibling and passes those *file paths* to unrpyc in chunks (Windows
   command-line length limit) — no `-c`, and unrpyc skips existing outputs on
   its own. An already-decompiled game is never rewritten again.
2. **Fail safe on the empty-`$` artifact.** `repair_and_track_decompiled`
   deletes any decompiled `.rpy` containing a line that is just `$` — the
   broken render of multi-line python from game-recompiled bytecode — so the
   game falls back to its own `.rpyc` instead of booting into a parse error
   (or silently losing the code). Deleted files are re-attempted on a later
   import, bounded to the missing set.
3. Regression tests: `missing_sources_targets_only_compiled_without_source`
   (unit) and `decompile_post_pass_deletes_broken_empty_dollar_renders`
   (integration, verbatim broken sample from this game).

## Rule

**Violated [[GAME-SAFETY]] rules 3 (never shadow an original — the clobber
replaced 300+ files that were fine) and 5 (validate what we wrote — exit 0
again proved meaningless).** Never run a whole-tree tool with clobber semantics
against a live game when only specific files are missing; decompile exactly the
missing paths. And a tool's "success" is never validation: after decompiling,
scan the output for the signatures we know are broken, and when a file can't be
trusted, remove it rather than leave it shadowing working bytecode. Also:
assume any `.rpyc` may have been recompiled by the game since our last
decompile — second-generation bytecode is not the developer's bytecode.
