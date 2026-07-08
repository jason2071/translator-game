---
title: AnvilNext `.Localization_Package` binary format (reverse-engineering)
aliases:
  - Localization_Package
  - AC loc binary
  - char-index format
tags:
  - type/research
  - engine/anvilnext
  - game/assassins-creed
  - reverse-engineering
status: in-progress
created: 2026-07-08
related:
  - "[[anvilnext-forger]]"
---

# AnvilNext `.Localization_Package` binary format (RE)

Working notes on reverse-engineering the Assassin's Creed **Origins** localization
binary, to let the app translate Origins end-to-end (the `.acod` engine in
[[anvilnext-forger]] only handles the *text* form the community Forger tool emits;
Origins ships no `.acod` — its text is in this binary). **Status: in progress —
the format is understood at the model level and confirmed crackable, but a
byte-exact decode+encode codec is not finished.**

## How the text is reached (external tools, user-run)

Origins has no `.acod`. To get at the text:

```
DataPC.forge ──Ubisoft_Forge_Tool -e──► NNN-LocalizationPackage_English*.data
             ──Ubisoft_DATA_Tool 11 -e──► 0-…_English*.Localization_Package  ← this file
```

Both are Delutto CLI tools (`GameCode 11 = Origins`, `12 = Odyssey`), symmetric
`-e`/`-i`. `aclocexport.exe` (community) decodes the package to text but appears
**decode-only** — no public encoder, which is the whole reason we need our own.

The round-trip the app must slot into: decode `.Localization_Package` → translate →
**re-encode** `.Localization_Package` → `DATA_Tool -i` → `Forge_Tool -i` → replace
`DataPC.forge` (backup first). No Forger patch needed — that's exactly how the
existing Thai Origins mod ships.

## How the existing Thai Origins mod was made (the diff that proved it)

Extracted the game's `DataPC.forge` and the Thai mod's `DataPC.forge`, then diffed
all 34 `*LocalizationPackage*.data`. **Only two changed**, both English slots:

| package | game (EN) | mod (TH) |
|---------|-----------|----------|
| `393-LocalizationPackage_English_Subtitles.data` | 378 KB | 645 KB |
| `401-LocalizationPackage_English.data` | 257 KB | 428 KB |

So the modder **overwrote the English slot with Thai** (play with language =
English → see Thai), didn't add a new language, and repacked the whole forge. This
gives a **parallel EN↔TH corpus** of the same package — the key RE lever.

## Format findings (from the EN↔TH parallel corpus)

`0-…_English_Subtitles.Localization_Package`: EN 404 717 B, TH 645 310 B.

- **Header:** first **37 bytes identical** across EN/TH. Magic `01 ED E0 6C`
  (`u32 0x6CE0ED01`), then `u32 = 23`, a resource id (`00 6F 9C 3C 6E …`), and
  small constants. Structural constants **identical in both** (so not text-derived):
  `u32[12]=366`, `u32[16]=256`, `u32[28]=248`. The header u32s from offset ~36 on
  differ (section pointers/sizes that grow with the larger TH payload).
- **Character table (dictionary):** distinct characters are stored as `u16`
  codepoints. EN uses **95** ASCII codepoints (`space..Z`, punctuation); TH uses
  **125** Thai codepoints (the full `U+0E00–0E7F` block) plus ASCII. Adding Thai =
  extending this table, which is a big chunk of the +240 KB growth.
- **Strings = arrays of `u16` indices** into the character table (small values seen
  right after the header: `2c 25 03 44 …`).
- **String offset/id table:** a run of `u32` at ~offset 76 372 (EN) reads as pairs
  of `u16`: the low half is a **sequential id/index** (`0x1970, 0x1971, 0x1972 …`),
  the high half rises irregularly (`0x1A51, 0x1A59, 0x1A65 …`) — consistent with a
  **cumulative offset / per-string length** table.
- **`0xF0–0xFF` bytes dominate** the string-data region (EN has 132 596 bytes
  `≥0xF0`) — very likely the **variable-length index encoding** (high-nibble escape
  / run scheme) into the character table. Nailing this scheme is the crux.

## What's left (the crux)

1. Pin the header section layout: where the char-table offset/count and the
   string-table offset/count live (the differing u32s from ~offset 36).
2. Decode the char table exactly (length, order — usage order vs sorted).
3. Decode the `0xF0+` variable-length index encoding → recover each string's text.
   Verify by round-tripping the EN file's decode against a known-good reference.
4. **Encode**: rebuild table + re-index a translated string set → byte structure
   the `DATA_Tool -i` (and the game) accept. This is the risk — a mismatch crashes
   the game or drops text (the community warns of line-feed / missing-text bugs).

Approach: use the parallel EN↔TH corpus — same string ids, different text — to
align each id's index sequence in EN vs TH and infer the encoding, rather than
guessing from one file.

## Feasibility / decision

🟡 **Crackable, but a real multi-session RE effort**, not a clean engine drop-in —
it's effectively a mini re-implementation of the Delutto/aclocexport codec plus an
encoder that doesn't exist publicly. If it lands, it becomes a heavy binary engine
(its own module, not the byte-span text seam). Parked mid-way here; the parallel
corpus and these findings are the restart point.

## Reproduce / continue

Samples on disk (this machine): `E:/Games/ac-loc/…English_Subtitles.Localization_Package`
(EN) and `E:/Games/ac-mod-loc/…English_Subtitles.Localization_Package` (TH). The
analysis scripts (`crack*.py`) live in the session scratchpad.

## See also

- [[anvilnext-forger]] — the `.acod` text engine (shipped) + the broader Forger pipeline
- [[games]] — game-translation research index
