---
title: Wolf RPG Editor — `.wolf` archives + `.mps`/`.dat` text
aliases:
  - Wolf RPG
  - WolfRPG
  - ウディタ
  - .wolf
  - WolfPro
tags:
  - type/research
  - engine/candidate
  - engine/wolfrpg
  - game/sannoji
status: removed
feasibility: medium-with-external-tools
created: 2026-07-28
related:
  - "[[games]]"
  - "[[ENGINES]]"
  - "[[ROADMAP]]"
---

# Wolf RPG Editor — `.wolf` archives + `.mps`/`.dat` text

> [!warning] The engine was removed
> A `wolfrpg` engine over a WolfTL dump was built, validated against a real
> 330-file dump (47 848 → 10 633 units after the rules below) and then **removed at
> the user's call** before it was ever patched back into a game. Everything here
> stays as the format record — the crypt analysis, the dump layout and the
> extraction rules are what a second attempt would start from.

Research notes for adding a **Wolf RPG Editor** (ウディタ) engine. Written after
inspecting a real shipped game on disk:
`F:\Downloads\otomi\山王寺家の人々～淫蕩の儀に狂ふ華～_v1_0_f2` (Jan 2025 build).

> [!tip] TL;DR
> Wolf's text lives in binary `.mps` (maps) / `.dat` (common events, database)
> files packed into encrypted `.wolf` DXArchives. **The archive crypto is the whole
> problem** — this game uses Wolf **crypt version 331** (v3.31), where the DXA
> header addresses are themselves AES-encrypted with a per-version key. Once
> unpacked, the community tool **WolfTL** already turns the binary text files into
> JSON and patches them back, which maps 1:1 onto this app's JSON-Pointer engine
> model (the same shape as [[anvilnext-locpackage-format]]: let an external tool
> own the binary, own the text format ourselves).

## Why Wolf is worth doing

- Wolf is the second-biggest Japanese doujin RPG toolkit after RPGMaker — a large
  share of untranslated JP indie/H-games ship on it.
- The game keeps its font **outside** the archives (`GenEiLateMin_v2.ttc` at the
  game root, referenced by name), so the `embed_font` story is a plain file swap —
  far easier than a Unity-style asset swap.
- Text is Shift-JIS-ish `tString` inside the binaries; WolfTL already emits UTF-8.

## What a shipped game looks like

```
山王寺家の人々…/
  Game.exe          8.0 MB    (Wolf runtime)
  Config.exe
  Game.ini                    plaintext [DEFAULT] Start=0 / WindowModeFlag=…
  GuruguruSMF4.dll            MIDI playback
  GenEiLateMin_v2.ttc  9.1 MB the game's font, loose on disk (OFL)
  LICENSE.txt / readme.txt
  Data/
    BasicData.wolf     2.2 MB  ← CommonEvent.dat, Game.dat, DataBase .project/.dat
    MapData.wolf       137 KB  ← *.mps (all map events)
    SystemFile.wolf    226 KB  ← UI strings
    Picture.wolf       273 MB / SE.wolf 376 MB / Voice.wolf 584 MB / BGM.wolf 78 MB …
```

Text-bearing archives are the three small ones. The other nine are media.

## `.wolf` = DXArchive (DxLib) ver 8

Header (`DARC_HEAD`, 64 bytes, `#pragma pack(1)`):

| Offset | Size | Field |
|-------:|-----:|-------|
| 0 | 2 | `Head` — `"DX"` = `0x4458` |
| 2 | 2 | `Version` — `0x0008` |
| 4 | 4 | `HeadSize` |
| 8 | 8 | `DataStartAddress` |
| 16 | 8 | `FileNameTableStartAddress` |
| 24 | 8 | `FileTableStartAddress` |
| 32 | 8 | `DirectoryTableStartAddress` |
| 40 | 4 | `CharCodeFormat` — **932** (Shift-JIS) in this game |
| 44 | 4 | `Flags` |
| 48 | 1 | `HuffmanEncodeKB` |
| 49 | 14 | `Reserve` — **doubles as the password/salt for the new crypt** |
| 63 | 1 | padding |

Baseline DXA v8: index block is Huffman- then LZ-compressed and XORed with a
**7-byte key** derived from a key string via two CRC32 passes; the key string is
what differs per Wolf version.

**`Flags >> 16` is the Wolf crypt version.** Our three archives all read
`Flags = 0x014B0000` → crypt version `0x14B` = **331** = Wolf **v3.31**. Known
markers, straight from UberWolf's table: `0x12C` v3.00, `0x13A` v3.14, `0x14B`
v3.31, `0x15E` v3.50, `0x64`/`0xC8` = the ChaCha20 variants.

For crypt version **≥ 331** (`g_newCrypt`) the reader must, *before* anything else:

1. `cryptAddresses(&Head, Head.Reserve, cryptVersion)` — decrypt the four address
   fields in place (they are garbage until then; we confirmed this on disk: the
   raw `DataStartAddress` reads `0x035E6AE5095A714B`, plainly nonsense, while the
   plaintext neighbours `CharCodeFormat=932` and the version marker are readable),
2. `initWolfCrypt(...)` + `initAES128(roundKey, Head.Reserve, …)` and decrypt the
   body in blocks, seeded through an MT19937 / MSVC-`rand` chain and SHA-512.

So a v3.31 archive is **not** openable by the plain DxLib/Python DXA readers.
Verified locally: `Wolf_RPG_Decompyler`'s `DXArchive.py` with the v2.281 / v3.00 /
v3.14 / default keys all reject the header, exactly as predicted by the marker.

**WolfPro** ("Pro Editor") adds another layer on top — a *protection key* and
per-file encryption whose key is derived from `Game.dat`'s own bytes
(`wolf::crypt::dxarckey`, `wolf::crypt::datadecrypt`). Detection and cracking of
that is what UberWolf's `WolfPro.cpp` / `WolfX/Crack.hpp` exist for.

## The text formats (after unpacking)

```
Data/BasicData/Game.dat        title, menu strings, misc game config
Data/BasicData/CommonEvent.dat all common events (the bulk of the script)
Data/BasicData/*.project/.dat  DataBase (items, actors, custom tables)
Data/MapData/*.mps             one file per map: events → pages → commands
```

A `.mps` is a length-prefixed binary: map header → events → pages → a command
list. Each command is `{ code, intArgs[], stringArgs[] }` — the same shape as
RPGMaker's `{ code, parameters }`, which is why this fits our model so well.
Text-bearing codes (WolfTL's table):

| Code | Command | Carries |
|-----:|---------|---------|
| 101 | Message | the dialogue line |
| 102 | Choices | choice options |
| 103 | Comment | dev note (skip) |
| 106 | DebugMessage | debug (skip) |
| 122 | SetString | string variable — often shown text |
| 210/211 | CommonEvent(Reserve) | args may hold shown text |
| 300+ | Picture/Sound/… | file names (skip) |

## Integration options

The binary + crypto is the cost; the text layer is cheap. Three shapes, in
increasing order of work:

### A. Consume a WolfTL dump (smallest) — **shipped**

The user runs **UberWolfCli** (unpack `.wolf`) then **WolfTL create** (binary →
JSON), and points this app at the dump. Our engine reads `dump/mps/*.json`,
`dump/db/*.json`, `dump/common/*.json`, `dump/*.json` (Game.dat) — each command
is already `{"code":101,"codeStr":"Message","stringArgs":["…"],"index":12}`, so
the pointer is a plain **JSON Pointer** (`/events/3/pages/0/list/12/stringArgs/0`)
and injection is `serde_json::Value::pointer_mut` — the exact MvMz path we already
have. Export writes the patched dump and the user runs `WolfTL patch`.
*Cost: ~an engine file + tests. Downside: two manual tool runs per game.*

### B. Bundle the tools as sidecars (the unrpyc pattern)

Same as A, but the app drives `UberWolfCli.exe` + `WolfTL.exe` itself, the way
Ren'Py drives the vendored `unrpyc`.
Both tools are **MIT** (Sinflower), C++/MSVC, and would have to be **built from
source by us** rather than shipping someone's release binary.
*Cost: A + a build script + bundling. Downside: another vendored toolchain.*

### C. Port decrypt + parse into Rust (self-contained)

Port `WolfCrypt` (address crypt, AES-128, SHA-512, MSVC-rand/MT19937 chains,
ChaCha20 variant), DXA v8 (Huffman + LZ), and the `.mps`/`.dat` parsers. Gives a
true byte-span engine with no external anything, and would also let us *repack*.
*Cost: ~4 000 lines of C++ to port, plus every future Wolf crypt version.*

**Recommendation: start at A, and only move to B once the text layer is proven
in-game.** A is a few days' work and de-risks everything downstream; C is a
standing maintenance burden (each new Wolf release changes the crypt).

## What shipped (option A)

`engine/wolfrpg.rs`, id **`wolfrpg`**. The user opens the WolfTL **output folder**
(the one holding `dump/`); pointing straight at `dump/` works too.

- **Detect** — WolfTL's own layout: a non-empty `dump/mps|common|db` or a
  `dump/Game.json`. A random folder of JSON never matches. Import shows a warning
  saying export writes the dump and `WolfTL … patch` is the last step.
- **Extract** — pointer is a **JSON Pointer**; `file` is the dump-relative path
  (`mps/Map001.json`). What counts as text was **rewritten against a real dump**
  (山王寺家の人々, 330 files) — the first pass produced 47 848 units, of which ~37 000
  were noise:
  - **A multi-language DB table is the whole script.** That game keeps its narrative
    in a `翻訳テキスト` DB type: 10 000 rows × `言語_1..言語_10`
    (JP/EN/ZH-S/ZH-T/KO/ES/FR/DE/PT/RU). Translating every column meant translating
    nine languages nobody asked for. Now `language_columns` recognises such a type,
    `pick_source_column` picks the best **source** by sniffing each column's script
    (English > Japanese > Chinese, via `engine::source_lang_rank`), and the unit's
    pointer targets **column 1** — the language the game shows by default — so the
    player sees Thai without touching the language menu. 9 584 units, English source.
  - **Wolf passes lookup keys as Japanese prose.** `250 Database` is
    `[_, type, row, field]`: four names locating a DB cell, one of which can read
    exactly like a line of dialogue (a row named `一度付けたら外せない？`).
    Translating one breaks the lookup, so the command is excluded whole — 4 376
    strings gone. `300 CommonEventByName` is `[event name, arg, …]`, so it starts at
    slot **1**: the name is a key, the args are what that event shows.
  - **Commands are an allowlist** (`arg_rule`), not a catch-all: 101 → Dialogue and
    102 → Choice unconditionally (so a one-word English choice isn't mistaken for an
    identifier), 122/210/211/300 → Message through
    `codes::looks_like_player_text`. Everything else — comments (103), debug (106),
    labels (212/213), picture/sound (150/140) — is dev- or engine-facing. The old
    catch-all tier alone was ~6 000 units of internal names.
  - **Code-only strings are skipped**: `[\cself[21]]\cself[7]` prints a variable
    and holds no prose (`is_only_codes`, via `protect::strip_codes`).
  - Database rows otherwise give their **`value`** (`Term`, context = the field name),
    skipping ints and WolfTL's `INVALID_IGNORE`; `Game.json` gives
    `Title`/`TitlePlus`/`StartUpMsg`/`TitleMsg` but never `MainFont`/`SubFonts`.
  - Editor-side labels (type / field / event / common-event names) are never taken.

  Result on that game: **47 848 → 10 633 units** (9 825 dialogue, 586 database terms,
  187 UI messages, 34 choices).
- **Inject** — `serde_json::Value::pointer_mut`, then re-serialize with a 4-space
  pretty printer and unescaped UTF-8, i.e. `nlohmann::json::dump(4)` — so a
  patched dump diffs cleanly against a freshly created one.
- **Round-trip identity has one exception**: a language-table unit *reads* one column
  and *writes* another, so injecting `translation == source` deliberately copies the
  English text into the default-language column. Every other file round-trips
  byte-identically.
- **Masking** — `mask_for("wolfrpg")` = the stock `mask()`: Wolf shares RPGMaker's
  backslash grammar (`\c[1]`, `\v[3]`, `\cself[5]`, `\udb[1:2:3]`, `\E`, `\>`) but
  **not** the MV/MZ angle-tag variant, since `<…>` is prose in Wolf. Mirrored in
  `src/codes.ts` + `src/messageWidth.ts` as `WOLF_RE`.
- **Tests** — `tests/wolfrpg_roundtrip.rs` over a committed dump fixture
  (`tests/fixtures/wolftl-dump/`) plus unit tests in the module.

### The font hook

Wolf resolves fonts **by family name**, not by path. Per the official help, a game
may ship `.ttf`/`.ttc`/`.otf` files **beside `Game.exe`** (or inside an unencrypted
`Data/`); Wolf registers them at startup and `Game.dat` stores the *name* to use,
falling back to ＭＳ ゴシック when that name isn't available. Our sample game does
exactly this — `GenEiLateMin_v2.ttc` sits loose at the root.

So `embed_font` is two halves:

1. copy `Sarabun-Regular.ttf` into the game folder, and
2. set `MainFont` — and every **non-empty** `SubFonts` slot, since an empty slot is
   an unused alternate — to `Sarabun` in the dump's `Game.json`.

Half 2 only reaches the game through `WolfTL … patch`, exactly like the
translation; the note says so. Both halves are idempotent (constant font name, and
re-writing the same TTF), so a re-export reproduces the same output.

Finding the game is the wrinkle: the dump normally lives outside it and nothing in
the dump records where it came from. If the project root (or its parent) looks like
a Wolf game — a `Data/` folder next to `Game.exe`/`GamePro.exe` — the TTF goes
straight there; otherwise it is written beside the dump and the note asks the user
to copy that one file. Running WolfTL with the game folder as its output
(`WolfTL <game>\Data <game> create`) makes the automatic path the normal one.

Mod export is refused for this engine with an actionable message: the project is a
dump, so a zip of it would be a folder of JSON no player can install.

## Fonts

The game loads `GenEiLateMin_v2.ttc` from the game root by name (declared in the
readme, OFL-licensed). Thai support is therefore a **file-level swap** to the
bundled Sarabun (`engine::TARGET_FONT`) plus whatever name the game config points
at — closer to the RPGMaker path than an asset-swap. Needs confirming against
`Game.dat` / `Config.exe` once we can read them.

## Status / open questions

- [x] Archive family + version identified (DXA v8, Wolf crypt **331**).
- [x] Text containers + command model identified (WolfTL).
- [x] Option **A implemented** — `wolfrpg` engine over a WolfTL dump, tests green
      against a committed fixture.
- [x] Run against a real dump: UberWolfCli + WolfTL were built from source (MSVC
      toolset **v145**; UberWolfCli needs its SelfUpdater include patched out, as
      that pulls ATL) and unpacked the sample game's crypt-331 archives, then dumped
      330 JSON files. The extraction rules above were rewritten from what that dump
      actually contains.
- [ ] Still unverified **in-game**: nothing has been patched back with
      `WolfTL … patch` and launched yet.
- [x] Font hook shipped — TTF into the game folder (when the dump sits in one) +
      `MainFont`/`SubFonts` → `Sarabun` in `Game.json`.
- [ ] Font not yet confirmed in-game: Wolf's family-name lookup is documented but
      untested here, and a `.ttc`-shipping game may need its own family removed
      first if Wolf prefers it.
- [ ] Is this specific game **WolfPro**-protected on top of v3.31? (Marker says
      plain v3.31 crypt; Pro detection needs `Game.dat`, which is inside the
      archive.)
- [ ] Does `WolfTL patch` output load in an unmodified Wolf runtime when the
      original archives were encrypted? (WolfTL's README says patched files are
      written **unencrypted**, and Wolf loads loose files over archives — needs a
      real in-game check.)

## Sources

- Sinflower — [UberWolf](https://github.com/Sinflower/UberWolf) (`WolfDec.cpp`
  key/marker table, `DXArchive.cpp` new-crypt branch), MIT
- Sinflower — [WolfTL](https://github.com/Sinflower/WolfTL) (binary ↔ JSON, the
  command-code table above), MIT
- Sinflower — [WolfDec](https://github.com/Sinflower/WolfDec) (older key list), MIT
- Daviid-P — [Wolf_RPG_Decompyler](https://github.com/Daviid-P/Wolf_RPG_Decompyler)
  (`DXArchive.py`, DXA v8 header + key derivation in readable Python)
- elizagamedev — WolfTrans (the original parsing work WolfTL is based on)

## See also

- [[games]] — research index
- [[anvilnext-locpackage-format]] — the same "external tool owns the binary" shape
