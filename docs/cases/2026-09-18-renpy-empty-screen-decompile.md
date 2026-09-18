---
title: "Ren'Py — empty screen decompiles to invalid script"
tags:
  - type/case
  - engine/renpy
created: 2026-09-18
status: repo fix implemented 2026-09-18 (unreleased) — post-pass repair + tracking, regression tests in `renpy_roundtrip.rs`
---

# Ren'Py — empty screen decompiles to invalid script

**Game:** City Lights & Love Bites — Season 1 (compiled-only Ren'Py 8.5.2,
translated to Thai via `tl/thai/` + `zzz_translator.rpy`).

**User-visible failure** — game refused to boot after import + export:

```
File "game/season1/screens/conscious_imagination_replay_controls.rpy", line 1: screen statement expects a non-empty block.
    screen ci_replay_transport_controls()
           ^
Ren'Py Version: Ren'Py 8.5.2.26010301
```

## Root cause

1. The game ships that screen as a genuine **empty stub**: in the `.rpyc`
   (both slots) `renpy.sl2.slast.SLScreen.children == []`.
2. unrpyc v2.0.3 (vendored at `resources/unrpyc/v2`) renders an empty screen
   as a bare `screen name()` header with **no block** — invalid Ren'Py. It
   exits 0 and reports "successfully decompiled"; `--try-harder` produces the
   same broken output.
3. `run_unrpyc` (`src-tauri/src/engine/renpy.rs`) trusts the exit code only,
   so import wrote the invalid `.rpy` next to the good `.rpyc`. Ren'Py loads
   whichever file is newer → the broken source shadows the original bytecode.

**Trigger:** any compiled-only Ren'Py game containing a screen (or other block
statement) with an empty body. Re-importing after the `.rpy` was deleted
recreates the broken file — `needs_decompiled` sees a `.rpyc` with no `.rpy`
sibling and decompiles again.

## Game-side fix (applied 2026-09-18)

Delete the invalid `.rpy`; the original `.rpyc` is intact and Ren'Py loads it:

```bat
del "<game>\game\season1\screens\conscious_imagination_replay_controls.rpy"
```

Verify by launching the game: `log.txt` shows "Loading script took … ms" and
`errors.txt` is not rewritten. Scanned all other decompiled `.rpy` for the same
truncation pattern — this was the only one. (`style x is y` and
`style.foo.bar = …` one-liners are valid Ren'Py; don't touch them.)

**Diagnosis recipe** (never run unrpyc on files inside the live game — it
writes its output next to the input):

```bat
mkdir "%TEMP%\case-test" & copy <game>\...\file.rpyc "%TEMP%\case-test\"
cd /d "%TEMP%\rpgtl-unrpyc\<app-version>\v2"
<game>\lib\py3-windows-x86_64\python.exe unrpyc.py -c --try-harder "%TEMP%\case-test\file.rpyc"
```

To dump the AST behind a screen: unpickle slot 1/2 with
`decompiler.renpycompat.pickle_safe_loads` and walk
`stmts[0].block[0].screen` (`renpy.sl2.slast.SLScreen`).

## Repo fix (implemented 2026-09-18, unreleased)

`ensure_decompiled` now runs `repair_and_track_decompiled` after a successful
`run_unrpyc`: it walks the decompiled `.rpy` (marker-guarded, `tl/` skipped),
repairs the empty-screen artifact to `screen name():` + `pass`
(`repair_decompiled_rpy`), and records every decompiled file in
`.rpgtl/decompiled.txt` so `restore_original` can undo the decompile.
Regression tests: `repair_decompiled_rpy_*` unit tests +
`decompile_post_pass_repairs_empty_screens_and_tracks_them` in
`tests/renpy_roundtrip.rs`.

Alternative (rejected): patch the vendored unrpyc to emit `pass` for empty
`SLScreen.children` — fixes every empty-block kind at the source but diverges
from the pinned upstream (would need documenting in
`resources/unrpyc/README`).

## Rule

**Violated [[GAME-SAFETY]] rules 3 (never shadow an original) and 5 (validate
what we wrote).**

Never trust unrpyc's exit code as "the output is valid Ren'Py" — it exits 0 on
uncompilable output. Anything that writes files into a game dir must
post-validate what it wrote (or be proven byte-safe). When a game breaks after
export, suspect our own written files first: look for the
`# Decompiled by unrpyc` marker and `.rpy` newer than its `.rpyc`.
