---
title: Cases — games broken by our pipeline
tags:
  - moc
  - type/case
created: 2026-09-18
---

# 🚨 Cases — games broken by our pipeline

One file per real incident where extract / decompile / export / inject broke a
game the user then had to repair by hand. **Read these before changing
`engine/`, export, or inject code** — every file here cost a real user a
broken game. `AGENTS.md` points here first for a reason.

## Index (newest first)

- [[2026-09-19-renpy-duplicate-strings-block]] — after the menu-caption
  pipeline shipped, a hand-written interim strings file collided with the
  app's own `rpgtl_menus.rpy`: the same `old` key declared twice is a
  boot-fatal duplicate in Ren'Py. Delete interim tl files before the first
  export that replaces them.
- [[2026-09-18-renpy-fontgroup-tofu]] — after export, every Thai string
  rendered as tofu squares: the game's text styles use FontGroup *objects*
  (CJK face + Latin default) and our font transform skipped non-string fonts,
  so Thai fell to a face with no Thai glyphs. Silent — no error log.
- [[2026-09-18-renpy-redecompile-clobber]] — re-opening a project re-ran the
  decompiler with clobber over the whole game dir; decompiling the game's own
  *recompiled* bytecode produced broken multi-line python (empty `$`) in 4
  files and the game wouldn't boot again.
- [[2026-09-18-renpy-empty-screen-decompile]] — unrpyc writes an invalid `.rpy`
  (screen header, no body) for a game's empty stub screen, exits 0 anyway;
  game won't boot because the broken `.rpy` shadows its `.rpyc`.

## How to record a case

Whenever the user pastes a game error log (typically `[code] … [/code]`):

1. **Diagnose** the root cause — reproduce on a *copy* of the game file
   (sandbox), never by running tools against the live game.
2. **Fix the user's game** and verify it boots (launch it; check `errors.txt`
   was not rewritten).
3. **Record** `docs/cases/YYYY-MM-DD-<engine>-<slug>.md` using the template
   below, add it to the Index above, and update `status` when the repo fix
   ships.
4. **Ship the repo fix with a regression test.** A case only flips to
   `fixed-in-<version>` once `cargo test` covers it — a test that fails
   without the fix (fixture, broken sample, whatever the engine's test file
   already uses). A case closed without a test will reopen.

```markdown
---
title: "<engine> — <one-line symptom>"
tags:
  - type/case
  - engine/<engine>
created: YYYY-MM-DD
status: open | repo-fix-proposed | fixed-in-<version>
---
# <engine> — <one-line symptom>

**Game:** <name, engine version, what our app did to it>

**User-visible failure** — <verbatim error log>

## Root cause
<numbered chain: what the game shipped → what our code wrote → why the game died;
 include the trigger condition + whether it recurs on re-import>

## Game-side fix (applied YYYY-MM-DD)
<exact commands to repair the user's game + how to verify it boots>

## Repo fix
<the permanent fix in this repo, or "proposed, pending approval" + the plan>

## Rule
<the one-paragraph lesson future agents must follow so this class of bug
 never ships again>
```

Keep cases self-contained: a future agent reading only this file must be able
to recognize the symptom, repair the game, and avoid reintroducing the bug.
