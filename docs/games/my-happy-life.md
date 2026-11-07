---
title: MyHappyLife 1.9.0 — DKTools quest localization
tags:
  - type/game
  - engine/rpgmaker-mvmz
created: 2026-10-18
---

# MyHappyLife 1.9.0

The MV game uses `DKTools_Localization` and `YEP_QuestJournal`. Quest display
parameters in `www/js/plugins.js` contain Chinese `{key}` references, including
110 configured quests. DKTools resolves those references from the selected
locale at draw time. Translating only `www/data/*.json` leaves the journal in
English, even when the dialogue has been translated.

## App support

The MV/MZ adapter now reads the enabled DKTools plugin's configured locale
folder. It extracts string values from the chosen source locale, supporting
both `<folder>/<locale>/*.json` and `<folder>/<locale>.json`. Auto prefers
English, then the configured primary locale; an explicitly unavailable source
language produces an error. Lookup keys and plugin configuration are preserved.

For this game, choose **English** as the project's source language. Open the
existing project and **Rescan** to add the locale units while retaining previous
translations. Translate those units, then Export (or Export Mod). Select
**English in the game** to display the exported Thai values; this adapter
replaces the source locale rather than registering another language.

There are 10,944 existing English dictionary strings. One additional literal
quest key (`夺回被抢走的保护费`) is absent from the shipped English tables.
The adapter extracts that fallback key and adds its translated value to an
existing object dictionary only on Export. Identity injection does not add it.
Fallback discovery is restricted to enabled YEP quest display fields; complex
lodash paths are not synthesized. Existing nested values and array entries are
still extracted normally.

## Preservation and regression coverage

- Locale values use RFC-6901 pointers. Replacement preserves BOM, CRLF,
  unrelated escapes and bytes; malformed/stale pointers or changed control
  codes fail before that locale file is written.
- DKTools lookup references inside dictionary values are protected as whole
  tokens during AI translation and subsequent typography cleanup.
- Locale files and plugin configuration participate in pristine rescan mirrors,
  backups, idempotent re-export, Restore Original and Export Mod staging.
- MV sibling files are staged under `www/`, fixing the old rescan mirror's
  root-level placement. Player saves and locale preferences are not exported.
- `tests/mvmz_dktools_roundtrip.rs` covers both MV/MZ layouts, directory/single
  file locales, custom folders, language selection, missing quest keys,
  byte-exact identity, tag validation, rescan, re-export, restore and mod ZIPs.

Initial adapter validation used a disposable copy of the game and a consistent
SQLite backup. It left the original game and translation project untouched.

On 2026-10-18, the copied project gained 10,945 units and retained all 20,920
previous rows with their original sources, translations and statuses. Two
exports were identical; a subsequent rescan added/removed zero rows. Hashes of
548 original game files and the original project's rows were unchanged.
`cargo test` passed 390 tests (including eight DKTools regressions),
`cargo build` emitted no warnings, and `pnpm build` passed.

Starting the sandbox's `Game.exe` for a boot/save smoke check was rejected by
automatic approval review with `blocked by policy`. Startup, save loading and
the rendered quest screen therefore remain unverified.

## Thai result guard (follow-up)

A subsequent real translation run returned four Chinese UI labels and one
Thai sentence with a stray Han character. The project target was correctly
set to Thai. Those five results were accepted as Translated and cached in TM;
the original TM snapshot did not contain the four UI mappings before the run.
The missing boundary was output-script validation, not locale selection.

`ai::validation` now rejects unprotected Han, kana and Hangul in Thai output.
It is deliberately not a general language detector: Latin names, HP/MP,
numbers, punctuation and protected lookup keys remain allowed. Other target
languages retain their existing behavior. The same guard runs before storing
AI results, during glossary-term translation, when writing TM, and when reusing
TM or translated siblings. Rejected Run items become Failed and are not cached;
valid neighbors are retained without automatically issuing extra AI requests.

With user approval, a consistent backup was saved to
`.rpgtl/repairs/2026-10-18-thai-language-guard-6015addd/project.db` before repair.
Only unit IDs 20933, 20937, 20938, 20943 and 22512 had their translations cleared
and status changed to Failed. Exactly five matching bad TM entries were deleted.
The other 31,860 unit rows, unrelated TM, glossary, characters and settings
were verified unchanged; SQLite quick_check passed. No runtime game or save
files were patched, and no AI retranslation was started by the repair.

Regression coverage includes the reported labels, one-character leaks,
protected CJK references, alternate targets, batched/split provider responses,
and poisoned TM/sibling reuse. The complete suite passed 397 tests, and
`pnpm build` passed.
