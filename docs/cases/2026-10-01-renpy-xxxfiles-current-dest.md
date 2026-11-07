---
title: "Ren'Py — xxxFiles ships an undefined current_dest in the bedroom menu"
tags:
  - type/case
  - engine/renpy
created: 2026-10-01
status: game repaired; boot checked; saved-game menu check pending; no app defect identified
---

# Ren'Py — xxxFiles undefined current_dest

**Game:** xxxFiles 0.3.1, Ren'Py 8.5.2.26010301, at
`F:\Downloads\xxxFiles-0.3.1-win`, with an app-generated Thai overlay.
This reported incident was investigated as a possible export regression, but
the offending code is present in the game's archive.

**User-visible failure** — selecting bedroom **Night companion → No one**:

```text
  File "game/exploration/interact/locations/house_interact.rpy", line 40, in <module>
    if current_dest == "bedroom":
       ^^^^^^^^^^^^
NameError: name 'current_dest' is not defined
```

The complete verbatim user traceback is preserved in
`.rpgtl/repairs/2026-10-01-current-dest-224523/original-traceback.txt` in this game.

## Root cause

1. Read the RPA-3.0 index and member bytes without extracting or writing into
   the game. Both `house_interact.rpy` (28,173 bytes) and its `.rpyc` (28,430
   bytes) were byte-identical to their respective members in `game/archive.rpa`
   before repair. Both conditions at lines 40 and 43 already use `current_dest`
   inside the archived source. Export did not change these files.
2. A search of the game's `.rpy` files found `current_dest` only in those two
   conditions. `exploration.rpy` declares `default selected_dest = "bedroom"`;
   `move_interact(dest)` assigns `selected_dest = dest`, and the other choices
   in the same bedroom menu use `selected_dest`.
3. Selecting **No one** evaluates the first undefined name and raises
   `NameError`. This is a gameplay variable mismatch already shipped in the
   archive, rather than a translation-string failure.

## Game-side fix (applied 2026-10-01)

User approved the concrete two-condition repair. Backed up both source and
bytecode to `.rpgtl/repairs/2026-10-01-current-dest-224523/`.
Changed only `current_dest` to `selected_dest` in the bedroom and ken-room
conditions; checked that reversing those two replacements exactly reconstructs
the backup bytes. Replaced the source through a temporary file and atomic rename.
No save files were edited.

Validation: the bundled Python/Ren'Py launcher completed `lint` with exit 0,
including unrelated game warnings (assets, phone groups, Python warnings).
Launched `xxxFiles.exe`: log reached `Loading script took`, `Running _start
took`, `Interface start took`, and initialized its renderer. No `errors.txt`
appeared; `traceback.txt` retained its original 22:40:53 timestamp and 223,902
byte size. This proves startup, not the saved-game interaction. Loading the
player's existing save and selecting **No one** remains a manual check.

To undo, restore the backed-up `.rpy` and `.rpyc` with the game closed.
That also restores the game's original bug. The archive is unchanged.

## Repo fix

No application code change is justified by this incident: the offending live
source and compiled file exactly matched the game archive. Do not make export
rewrite gameplay variables to work around this game-specific bug.

## Rule

An error after export is a reason to investigate the pipeline, not proof of
pipeline damage. Compare the responsible files against an independent shipped
archive or original backup before assigning blame. A direct gameplay repair
requires a specific user request, a backup, a minimal byte-preserving patch,
and honest separation of startup validation from saved-game verification.
