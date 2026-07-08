---
title: AnvilNext `.Localization_Package` — Origins text bridge
aliases:
  - Localization_Package
  - AC loc binary
  - aclocexport format
  - AC Origins text
tags:
  - type/research
  - engine/anvilnext
  - game/assassins-creed
  - reverse-engineering
status: implemented
created: 2026-07-08
updated: 2026-07-08
related:
  - "[[anvilnext-forger]]"
---

# AnvilNext `.Localization_Package` — Origins text bridge

How to let the app translate Assassin's Creed **Origins** end-to-end. The `.acod`
engine in [[anvilnext-forger]] handles the text form the community Forger tool
emits; Origins ships no `.acod`. Its text lives in a binary `.Localization_Package`
— **but that binary does not have to be reverse-engineered**, because the community
already ships a matched **decode + encode** pair (`aclocexport` / `aclocimport`)
that turns it into a plain UTF-8 text file and back. The app only has to translate
that text file. **Status: implemented — the `ac-loctext` engine
(`src-tauri/src/engine/ac_loctext.rs`) ships, format confirmed on 33 787 real
Origins records; detect/extract/round-trip/inject + protect + reexport tests green.**

> **Correction (was wrong before):** earlier notes here said `aclocexport` is
> "decode-only, no public encoder", concluding we had to build our own binary
> codec. That is false. **`aclocimport.exe` exists** and is the standard second
> half of the community workflow (`aclocexport` → edit `.txt` → `aclocimport` →
> `.txt.out`). So the binary codec is **not** on the critical path. The binary
> findings are kept below as an appendix only.

## The real pipeline (all external steps user-run)

```
DataPC.forge ──Forge_Tool -e──►  NNN-LocalizationPackage_*.data
             ──DATA_Tool 11 -e─►  0-…_*.Localization_Package     (binary)
             ──aclocexport──────►  LocalizationData.txt          ← APP TRANSLATES THIS
   [ app: extract → translate → export ]
             ──aclocimport──────►  LocalizationData.txt.out      (binary, re-encoded)
             ──DATA_Tool 11 -i─►  NNN-…_*.data
             ──Forge_Tool 11 -i►  DataPC.forge   (back up first, then overwrite)
```

`GameCode 11 = Origins`, `12 = Odyssey`; Forge/DATA tools are Delutto CLI,
symmetric `-e`/`-i`. `aclocexport`/`aclocimport` are the community text bridge
(referenced by the MOIX-1192 corpus repo; binaries distributed in modding
communities, not open-source). This is exactly how the existing Thai Origins mod
was produced — overwrite the **English** slot with Thai, repack the forge, play
with language = English → see Thai (confirmed by diffing the game vs mod forge:
only `393-…English_Subtitles` and `401-…English` changed).

## `aclocexport` text format (confirmed on real Origins data)

Verified against the real Origins `English_Subtitles` export (MOIX-1192 corpus,
`aco-…English_Subtitles.Localization_Package.txt`, **33 787 records**):

- **UTF-8, no BOM, CRLF** (`\r\n`) line endings.
- Record = an id line, the text line, then a blank line:

  ```
  Id: [0x000D1792]
  You must choose, Quick!
  <CRLF blank line>
  Id: [0x000D197F]
  How did you get past the guard? No one gets past the guard.
  ```

- id = `Id: [0x` + **8 uppercase hex** + `]`; ids are unique.
- **Every value is exactly one line** — 0 multi-line, 0 empty in the whole file. A
  literal newline inside a line is written as the markup token `<LF>` / `<CR>`, not
  an actual break. (So parse is trivial: id line, one text line, blank.)
- **Inline markup to protect (mask, never translate):**
  - angle tags `<i> </i> <b> </b> <LF> <CR>` (shape-based, same family as `.acod`)
  - square-bracket performance/audio cues: `[beat]`, `[&breath]`, `[&laughs]`,
    `[/&laughs]` (closing form has `/`), `[sigh]`, `[&scoff]`, `[&gasp]` …
- **Curly `{…}` is NOT a variable** here — it wraps a whole translatable line
  (e.g. `{I am looking to hire a <i>misthios</i>.}`), and is very rare (2 in
  33 787). Do **not** mask it away; keep the braces, translate the inside.
- **No real printf** (`%s/%d`) in the corpus (`% g` seen is prose "…% g…"), so
  printf masking should stay off for this engine to avoid eating prose `%`.

## The `ac-loctext` engine (implemented)

Slots into the existing text-engine pattern — **simpler than `forger_acod`**
because the file is already UTF-8 (no Shift-JIS/UTF-16 re-encode). As shipped in
`src-tauri/src/engine/ac_loctext.rs` (registered last in `engine::engines()`):

1. **detect** — content-based (extension `.txt` is generic): the file's **first
   line** must be an `Id: [0x<8-hex>]` header. So a stray `.txt` never matches.
2. **extract** — pairs each header with its following value line; `TransUnit`
   pointer = the **UTF-8 byte span** (`"start:len"`) of the value, context = the
   hex id, kind = Dialogue.
3. **inject** — splices the translation into that byte span (unchanged unit =
   byte-identical → round-trip identity is free, like Tyrano). Guards a stale
   pointer / non-char-boundary instead of panicking.
4. **protect** — `mask_ac_loctext`: shape-based `angle_tag_len` for `<…>`; masks
   `[…]` cue brackets; **leaves `{…}` and `%` alone**. Mirrored in `src/codes.ts`
   (`AC_LOCTEXT_RE`) + `src/messageWidth.ts` (`AC_LOCTEXT_CODE_RE`).
5. Tests: `tests/ac_loctext_roundtrip.rs` (detect, "a plain .txt isn't claimed",
   extract, byte-exact round-trip, targeted Thai inject) + inline engine/protect
   tests + `ac_loctext_reexport_is_idempotent` in `tests/reexport_idempotent.rs`.
   Full suite green (178), warning-free, `tsc` clean.

Font/glyph is **not** this engine's job — the game already renders Thai once the
Thai codepoints are in the package (the existing mod proves it); no font embed
needed on the app side for this path.

## Feasibility / decision

🟢 **Easy engine, high value** — the hard part (binary codec) is done by the
community tools; the app only owns a clean UTF-8 key/text format it already has
the machinery for. External-tool dependency (Delutto + aclocexport/aclocimport)
is the cost, same as the Forger path. This supersedes the binary-RE plan below.

---

## Appendix — binary `.Localization_Package` findings (not on the critical path)

Kept only in case a future no-external-tool path is ever wanted. From the parallel
EN↔TH corpus (`English_Subtitles`: EN 404 717 B, TH 645 310 B):

- **Header:** first 36–37 bytes identical across EN/TH. Magic `01 ED E0 6C`
  (`u32 0x6CE0ED01`), then `u32 = 23`, a resource id, small constants; structural
  constants identical in both (`u32[12]=366`, `u32[16]=256`, `u32[28]=248`). Header
  u32s from ~offset 36 differ (section pointers/sizes that grow with the payload).
- **Char table (dictionary):** distinct characters as `u16` codepoints — EN 95
  ASCII, TH 125 Thai (`U+0E00–0E7F`) + ASCII. TH's early bytes show Thai codepoints
  big-endian (`0e32`=า, `0e19`=น, `0e07`=ง, `0e40`=เ, `0e48`=่).
- **Strings = arrays of `u16` indices** into the char table.
- **id/offset table** ~offset 76 372 (EN): `u16` pairs, low half a sequential id
  (`0x1970,0x1971…`), high half a cumulative offset.
- **`0xF0–0xFF` dominate** the string region (EN 132 596 bytes `≥0xF0`) — the
  variable-length index encoding; decoding it was the unfinished crux. Not needed
  now that the text bridge is confirmed.

## Reproduce / continue

- Confirmed text format: MOIX-1192 corpus repo (`aco-…English_Subtitles…txt`), and
  the same shape holds for other AC titles in that repo.
- Binary samples on disk (this machine): `E:/Games/ac-loc/…Localization_Package`
  (EN) and `E:/Games/ac-mod-loc/…` (TH). Analysis scripts (`crack*.py`) in the
  session scratchpad.

## See also

- [[anvilnext-forger]] — the `.acod` text engine (shipped) + the broader Forger pipeline
- [[games]] — game-translation research index
