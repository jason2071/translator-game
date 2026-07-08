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
status: implemented
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
- `text` — the translatable value. May contain inline markup (see the inventory
  below), treated like the control/markup codes other engines already mask.
- **Line terminator is `CRLF`** (`0D 00 0A 00` in UTF-16LE) — confirmed on a real
  25,763-line Odyssey `UI.acod`, where 25,762 lines match `^[0-9A-Fa-f]{8}=`
  (one trailing empty line).

**Markup inventory (confirmed on a real `UI.acod`, by frequency).** Richer than
just `<font>` — the mask must handle all of these, and `{…}` variables are the
critical ones (never translate/corrupt a runtime substitution):

| Markup | Count | Meaning |
|--------|-------|---------|
| `<font face='…'>` / `<font size="…">` / `</font>` | ~8200 | font run |
| `{…}` | 2702 | **runtime variable** (player name, counts) — must survive verbatim |
| `<br/>` | 1297 | line break |
| `<style name='…'>` / `</style>` | ~2000 | styled run |
| `[…]` | 335 | bracketed token |
| `<img src='…'/>` | ~240 | inline button/icon glyph |
| `<i>` / `</i>` | 460 | italic |
| `%s` / `%d` | ~2 | printf slot |

Human-translated files also contain **malformed tags** (`</f</font>`, `< font …>`,
`<br/ >`) from translator typos — the mask regex must be lenient enough that a
stray `<…>`-ish run still masks/restores without dropping a sentinel.

Real samples (English source and shipped Thai):

```
07270E50=<font face='DINPro_Bold'>I wish I could retire.</font>
000D1792=เจ้าต้องเลือกแล้ว เร็วเข้า!
00093521=การบันทึกของคุณเสียหาย<br/>คุณต้องการเขียนทับและเริ่มเกมใหม่หรือไม่?
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

## Implementation plan

> [!done] Status — engine shipped (branch `engine-forger-acod`)
> Phases 1–4 are implemented and green (166 Rust tests, warning-free; `tsc`
> clean): `engine/forger_acod.rs` (`id = "forger-acod"`), `protect::mask_forger`
> + the `codes.ts` / `messageWidth.ts` mirrors, `tests/forger_acod_roundtrip.rs`,
> and a Forger case in `tests/reexport_idempotent.rs`. **Remaining:** validate
> against a real Forger-exported EN `.acod` (the blocker below) and confirm the
> Origins export extension/markup match the Odyssey-format fixtures.

Phased plan to add the `forger_acod` engine. Grounded in the confirmed format
facts above; the only real blocker is getting an English source `.acod` (see
below), which does **not** block engine dev — synthetic fixtures cover that.

### Phase 0 — Recon
- ✅ Format confirmed on real files: UTF-16LE + BOM `FF FE`, **CRLF**, `HEXID=text`,
  full markup inventory (`<font>`/`<style>`/`<br/>`/`<img>`/`<i>` + `{…}` variables
  + `[…]` + `%s`).
- ☐ Document how a user obtains an EN `.acod` from Forger (the workflow, below).

### Phase 1 — Engine core (`src-tauri/src/engine/forger_acod.rs`)
- `detect` — a folder holding ≥1 `*.acod` whose bytes start with BOM `FF FE` and
  whose lines match `^[0-9A-Fa-f]{8}=`.
- `describe` — count key lines (read-only).
- `extract` — one `TransUnit` per line; `source` = text after `=`; pointer =
  **byte span `"start:len"` into the UTF-16LE bytes** (the value span); mask markup.
- `inject` — splice the translation into the value span; keep UTF-16LE, CRLF, the
  `HEXID=` key, and the BOM.
- Reuse `engine/encoding.rs` UTF-16LE (`read_utf16le` / `encode_utf16`, from KiriKiri).
- Register in `engine::engines()` (`.acod` extension is unique — order-independent).

### Phase 2 — Protection (`src-tauri/src/engine/protect.rs`)
- `mask_forger`: mask **every `<…>` tag** (generic, lenient for malformed ones) plus
  `{…}` variables, `[…]`, and `%s`/`%d` → `⟦n⟧` sentinels.
- Branch in `protect::mask_for("forger_acod", …)` (shared `restore` is engine-agnostic).
- Mirror the inline codes in `src/codes.ts` + `src/messageWidth.ts` (grid warning).

### Phase 3 — Tests (`src-tauri/tests/forger_acod_roundtrip.rs`)
- In-test synthetic `.acod` builder (UTF-16LE + BOM + CRLF + `ID=text` + markup) —
  same approach KiriKiri uses to build its Shift-JIS/UTF-16 fixtures.
- `detect` / `extract` count / **`roundtrip_identity`** (unchanged = byte-identical)
  / targeted `inject` (span replaced, `HEXID` untouched) / mask-restore identity
  (including a malformed tag).

### Phase 4 — Export integration
- Detect a folder of `.acod` as a project; reuse the existing `.rpgtl/source/`
  snapshot + idempotent-export path (works unchanged for `.acod`).
- Guard with a `reexport_idempotent`-style test.

### Phase 5 — Docs + font (external)
- Flip this note `status: proposed → implemented`; `ENGINES.md` row → ✅ Supported.
- Write the end-to-end user flow: Forger export EN `.acod` → app translates →
  drop into `ForgerPatches/` → run Forger.
- Font: a one-time FontForge merge of Thai glyphs into the game font — **out of app
  scope**, documented for the user.

### Effort / sequencing
| Phase | Effort | Depends on |
|-------|--------|-----------|
| 1 Engine core | ~½ day | `encoding.rs` (exists) |
| 2 Protect | ~2 h | Phase 1 |
| 3 Tests | ~½ day | Phase 1–2 |
| 4 Export | ~2 h (reuse) | Phase 1 |
| 5 Docs | ~1 h | Phase 1–4 |

~1.5–2 focused days. Optional warm-up: move `ExtractOpts` out of `codes.rs` into
`engine/mod.rs` beside the trait (a coupling the knowledge-graph flagged — every
engine reaches into the RPGMaker module for an engine-agnostic options struct).

### Blocker + open question — the EN source `.acod`

- On-disk `.acod` files are the **already-translated Thai** (the endpoint). A fresh
  translation needs the **English source**, which comes from unpacking the game's
  English localization forge with Forger/Blacksmith — a GUI step the user runs; the
  app never touches the `.forge`.
- **Origins vs Odyssey caveat.** The `.acod` format is confirmed for **Odyssey**
  (real files). The on-disk **Origins** mod ships as a *full `DataPC.forge`
  replacement + `FontACO.rar`* (older Blacksmith flow), **not** Forger `.acod`
  patches. Investigation confirmed **Origins ships no `.acod` at all** — its text
  lives in a binary `.Localization_Package` that the Delutto CLI tools expose from
  the forge. **Resolved:** no binary codec needed — the community `aclocexport`/
  `aclocimport` pair turns that binary into plain UTF-8 `Id: [0x…]` text and back,
  and the shipped **`ac-loctext`** engine translates that text. See
  [[anvilnext-locpackage-format]] (implemented). So Origins uses `ac-loctext`;
  Odyssey/Valhalla use this Forger `.acod` engine.
- **Recommended:** build Phases 1–3 now against synthetic fixtures (Odyssey-validated),
  and swap in a real EN `.acod` fixture once one is exported — adapting the markup
  mask if Origins differs.

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
