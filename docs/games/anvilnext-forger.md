---
title: AnvilNext (Assassin's Creed) — Forger `.acod` localization
aliases:
  - AnvilNext
  - Forger
  - "AC Origins translation"
  - "AC Odyssey translation"
  - "AC Valhalla translation"
  - .acod
tags:
  - type/research
  - engine/candidate
  - engine/anvilnext
  - game/assassins-creed
status: proposed
feasibility: easy-with-external-tools
created: 2026-07-08
related:
  - "[[ENGINES]]"
  - "[[ROADMAP]]"
  - "[[games]]"
---

# AnvilNext (Assassin's Creed) — Forger `.acod` localization

Research notes + integration proposal for translating Ubisoft **AnvilNext** games
(Assassin's Creed **Origins / Odyssey / Valhalla**) via the community **Forger**
tool. Written after inspecting three real Thai fan-translation mods
(`Thai Subtitles By Mamol / มนล`) on disk.

> [!tip] TL;DR
> The raw game archive (`.forge`) is binary + Oodle-compressed and out of scope,
> **but the localization the community edits is a plain UTF-16LE `ID=text` string
> table (`.acod`)** that maps almost 1:1 onto this app's existing byte-span engine
> model. The app's realistic scope is **translating the `.acod`**; unpacking the
> forge and merging a Thai font stay external one-time steps done with Forger +
> FontForge.

## The games / format family

| Game | Engine | Archive magic | Mod distribution seen on disk |
|------|--------|---------------|-------------------------------|
| AC Origins | AnvilNext | `scimitar` `.forge` | full `.forge` overwrite + separate `FontACO.rar` |
| AC Odyssey | AnvilNext | `scimitar` `.forge` | **Forger patch** — `.forger2` + `.acod` in `ForgerPatches/` |
| AC Valhalla | AnvilNext | `scimitar` `.forge` | pre-repacked `DataPC_patch_02.forge` overwrite |

All three share the `scimitar` forge container and the same `.acod`-style
localization underneath; they differ only in **how the edited resource is shipped**
(direct forge replacement vs Forger's non-destructive patch folder).

`.forge` header (verified): magic `scimitar` (8 bytes) then version/table fields.
Entry payloads are **Oodle Kraken** compressed — decompression needs
`oo2core_7_win64.dll`, which ships with the game (and with the Forger utility).
This is why the forge itself is not something we parse directly.

## How the existing Thai mods are made

Reconstructed from the tools the mod author credits (installation-guide PDF,
"ขอขอบคุณโปรแกรมที่เกี่ยวข้อง"):

1. **Blacksmith** / **Ubisoft_Data_Tool_By_Delutto** — unpack the `.forge`,
   decompress entries with the Oodle DLL, locate the localization resources.
2. Export localization to **`.acod`** — a UTF-16LE `ID=text` table (Forger's
   editable form; SUB = subtitles/dialogue, UI = menu strings, split per DLC).
3. **Notepad++ / HxD** — translate each line's text to Thai, keeping the `HEXID=`
   key and any inline `<font …>` markup.
4. **FontForge** (+ Thai fonts *Angsana* / *Crodia*) — merge Thai glyphs into the
   game's font resource so Thai actually renders. **One-time per game.**
5. **Forger** — read the `.forger2` JSON manifest, apply the edited `.acod`
   resources, recompress with Oodle. Odyssey uses a non-destructive
   `ForgerPatches/` folder that Forger applies at launch; older games get a
   repacked `.forge`.
6. **Nexus** — distribute.

### Correction: Thai *does* render on AnvilNext

An earlier assumption was that AAA engines can't shape Thai (stacked tone/vowel
marks, no word spaces) so injected Thai would be tofu. That is **wrong for
AnvilNext** — it ships **Arabic** as an official language, i.e. it already has a
real Unicode text shaper capable of complex scripts. Thai renders fine once the
**font resource contains Thai glyphs** (the FontForge step). No DLL hook or engine
patch is needed — the fix is data (font + strings), not code. A shipped mod on
disk confirms this: **26,646 of 33,790 lines** in one Odyssey subtitle `.acod` are
Thai and display in-game.

## The `.acod` format (what we would parse)

Plain **UTF-16LE**, BOM `FF FE`, one record per line:

```
<HEXID>=<text>
```

- `HEXID` — 8 hex digits, the string's stable id inside the localization resource
  (e.g. `000D1792`, `07270E50`). This is the join key Forger uses to place the
  string back into the forge.
- `text` — the translatable value. May contain inline markup, notably
  `<font face='DINPro_Bold'>…</font>`, treated like the control/markup codes other
  engines already mask.

Real samples (English source and shipped Thai):

```
07270E50=<font face='DINPro_Bold'>I wish I could retire.</font>
000D1792=เจ้าต้องเลือกแล้ว เร็วเข้า!
000D19DE=เจ้าใช่อันธูซ่ามั้ย
```

`.forger2` — the mod manifest, JSON, `{"Format": "ForgerPatch2", …}`. It ties the
`.acod` resources to their forge targets. **Read/written by Forger, not by us.**

## Proposed app integration

Add `.acod` as a new **text engine** (`src-tauri/src/engine/forger_acod.rs`). It
maps onto the invariants the codebase already enforces (see [[ENGINES]] and the
engine-adding pattern in [[ROADMAP]]) — almost nothing new:

| App invariant | How `.acod` uses it |
|---------------|---------------------|
| Byte-span `"start:len"` pointer | ✅ same model as Ren'Py/Tyrano/KiriKiri — pointer = the value span within the line |
| UTF-16LE output | ✅ already implemented for KiriKiri (`engine/encoding.rs`) — reuse |
| Mask inline markup around AI | ✅ add a `mask_forger` branch in `protect.rs` for `<font …>…</font>` |
| Windowed grid (~1M units) | ✅ 33k lines/file is trivial for the existing virtualized grid |
| Round-trip identity | ✅ unchanged unit splices back byte-identical (no re-serialize) |

`GameEngine` sketch:

- **`detect`** — extension `.acod`, BOM `FF FE`, lines match `^[0-9A-F]{8}=`.
- **`describe`** — count `ID=` lines (read-only).
- **`extract`** — one `TransUnit` per line; `source` = text after `=`; pointer =
  byte span of the value; mask `<font …>` markup.
- **`inject`** — splice the translation into the value span, keep UTF-16LE,
  preserve the `HEXID=` key and BOM.
- **`roundtrip_identity`** test — `extract → inject with translation == source`
  reproduces the original bytes exactly.

### Scope boundary (important)

The app translates the **`.acod`** and nothing binary:

- **In scope:** parse `.acod` → extract EN units → AI/hand translate → inject Thai
  → round-trip-safe `.acod`.
- **External, one-time, left to Forger/FontForge:**
  - Unpack the `.forge` and export `.acod` (Forger + Oodle DLL).
  - Merge Thai glyphs into the game font (FontForge) — once per game, reused.
  - Repack / apply the patch (Forger.exe). A later enhancement could shell out to
    Forger, but the clean seam is: **we produce the translated `.acod`, the user
    drops it into `ForgerPatches/` and runs Forger.**

This keeps the risky binary/compression/signature work with the tool built for it,
while the app does what it is good at — the string-level translate/round-trip.

## Feasibility

🟢 **Easy** for the `.acod` text layer (fits the byte-span model directly), with a
hard dependency on the **external Forger + FontForge** steps for everything around
it. Not a self-contained "open the game folder and export" flow like RPGMaker —
the user must run Forger first to get `.acod` and once to install a Thai font.

## Fixtures / tests (when implemented)

Real `.acod` pairs exist on disk (English source + the shipped Thai) and can seed
a committed fixture:

- `detect` on a `.acod` header,
- `extract` count vs a known line count,
- `roundtrip_identity` (splice unchanged == original bytes, BOM + UTF-16LE intact),
- targeted `inject` of one Thai line, re-read to confirm the span replaced and the
  `HEXID` key untouched,
- `<font …>` markup masks and restores cleanly (no sentinel loss).

## See also

- [[games]] — game-translation research index
- [[ENGINES]] — full engine translatability reference (AnvilNext row)
- [[ROADMAP]] — the generic engine-adding pattern this follows
