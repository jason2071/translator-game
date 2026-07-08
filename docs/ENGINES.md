# Game engine landscape — translatability reference

The engines below are the common tags used to categorize games (as seen on game
distribution sites). This maps each to **where its text lives**, whether that is
**text or binary**, and how feasible it is for this translator's model —
extract → edit → inject with **byte/semantic round-trip identity**. Text formats
fit our byte-span locator (like Ren'Py); binary formats need a hand-rolled codec
and are much riskier.

## Feasibility key
- ✅ **Supported** — implemented now
- 🟢 **Easy** — text format, fits the extract/inject + byte-round-trip model
- 🟡 **Medium** — text but structure/encoding varies, or a documented binary table
- 🔴 **Hard** — binary and/or encrypted; needs a custom codec + byte-exact writer
- ⚫ **Out of scope** — runtime-hook paradigm, not a file-extract problem

## Summary table
| Engine | Format / where text lives | Type | Feasibility |
|--------|---------------------------|------|-------------|
| **RPGM** (MV/MZ) | `data/*.json` (event lists, database, System) | text | ✅ Supported |
| **Ren'Py** | `game/**/*.rpy` (say / menu / `_()`) | text | ✅ Supported |
| **TyranoScript** | `data/scenario/*.ks` (message / glink / jname) | text | ✅ Supported |
| **KiriKiri** (KAG) | `*.ks` (same KAG tags, Shift-JIS/UTF-16) | text | ✅ Supported |
| **RPGM** (VX Ace/VX/XP) | `Data/*.rvdata2` = Ruby Marshal | binary | 🔴 Hard |
| **RPGM** (2000/2003) | `*.lmu` / `RPG_RT.ldb` (liblcf) | binary | 🔴 Hard |
| **Godot** | `.po` / `.csv` → `.translation`; `.tscn` scenes | text (mostly) | 🟢 Easy (if `.po`/`.csv`) |
| **HTML** | `.html` / `.js` (Twine/SugarCube, custom) | text | 🟡 Medium |
| **QSP** | `.qsps` source → `.qsp` compiled | text (source) | 🟡 Medium |
| **TADS** | `.t` source → `.gam` / `.t3` compiled | text (source) | 🟡 Medium |
| **Wolf RPG** | `Data.wolf` archive, `.mps` maps | binary, often encrypted | 🔴 Hard |
| **ADRIFT** | `.taf` (compiled, obfuscated) | binary | 🔴 Hard |
| **RAGS** | `.rag` (binary DB) | binary | 🔴 Hard |
| **Flash** | `.swf` (compiled ActionScript) | binary | 🔴 Hard (legacy/EOL) |
| **Java** | `.jar` (`.properties` or hardcoded in `.class`) | mixed | 🟡 Medium if `.properties`, else 🔴 |
| **Unity** | IL2CPP / Mono DLL, TextMeshPro, `resources.assets` | binary | ⚫ Out of scope (use XUnity) |
| **Unreal Engine** | `.locres` localization table | binary (documented) | 🟡 Medium |
| **AnvilNext** (AC Origins/Odyssey/Valhalla) | `.forge` archive → Forger-exported `.acod` (UTF-16LE `ID=text`) | binary archive / **text once exported** | ✅ Supported for `.acod` (needs external Forger + font) |
| **WebGL** | build target — usually Unity WebGL or HTML5 | — | see Unity / HTML |
| **Others** | catch-all | — | case by case |

## Notes per engine

### Supported today
- **RPGMaker MV/MZ** — JSON data files; pointer = RFC-6901 JSON Pointer;
  re-serialized compact for round-trip. `src-tauri/src/engine/mvmz.rs`.
- **Ren'Py** — `.rpy` scripts; pointer = byte span; splice-in-place inject; skips
  `game/tl/<lang>/`; protects `[interpolation]` / `{tags}`.
  `src-tauri/src/engine/renpy.rs`.
- **TyranoScript** — `.ks` KAG scenario scripts; pointer = byte span; splice-in-place
  inject. Extracts message text, `[glink text=]` choices, and `[chara_new jname=]`
  names; skips comments/labels/`@`-commands and `[iscript]`/`[html]` blocks;
  protects `[tags]`. UTF-8 only. `src-tauri/src/engine/tyrano.rs`.
- **KiriKiri (KAG)** — `.ks` scripts in **Shift-JIS/UTF-16** (or UTF-8). Reuses
  the TyranoScript KAG parser + `mask_tyrano` verbatim behind an encoding layer
  (`src-tauri/src/engine/encoding.rs`): decode-on-read, re-encode-on-write, so
  round-trip stays byte-exact. Detected by a `.tjs`/`.xp3` fingerprint (tried
  before TyranoScript). When a translation isn't representable in the source
  encoding (e.g. Thai in a Shift-JIS game) the file is written as UTF-16LE, which
  KiriKiri loads natively. `src-tauri/src/engine/kirikiri.rs`.
- **AnvilNext / Forger `.acod`** (Assassin's Creed Origins/Odyssey/Valhalla) —
  UTF-16LE `HEXID=text` string tables the community **Forger** tool exports from
  the game's `.forge` archives. Pointer = byte span into the decoded UTF-8;
  splice-and-re-encode-UTF-16LE (same shape as KiriKiri), so round-trip is
  byte-exact with BOM + CRLF preserved. `mask_forger` protects HTML-ish angle
  tags plus `{variable}`/`[bracket]`/`%s`. Unpacking the `.forge` and merging a
  Thai font stay external one-time Forger/FontForge steps.
  `src-tauri/src/engine/forger_acod.rs`; deep-dive in `docs/games/anvilnext-forger.md`.

### Text-based candidates (fit the model — recommended path)
- **Godot** — trivial when the game ships `.po`/`.csv` gettext catalogs; scene
  text in `.tscn` (text) is also parseable. `.translation` (compiled) is binary —
  prefer the source catalogs.
- **HTML** — Twine/SugarCube has a regular passage structure (`:: PassageName`);
  custom HTML/JS games vary per title. Feasible but per-engine heuristics needed.
- **QSP** — Russian text-quest; translate the `.qsps` source (plain text). If only
  the compiled `.qsp` ships, a decompiler is required first.
- **TADS** — text adventures; source `.t` is plain text. Compiled `.gam`/`.t3` is
  binary — needs source.

### Binary / hard
- **RPGMaker VX Ace / VX / XP** — same audience as our flagship MV/MZ, but
  `.rvdata2` is a Ruby **Marshal** dump. No mature Rust crate; requires a
  hand-rolled Marshal reader + writer with byte-exact output. Highest audience
  value, largest effort.
- **RPGMaker 2000/2003** — `liblcf` (LMU/LDB) format; reference impl is C++.
- **Wolf RPG** — assets packed in an often-encrypted `Data.wolf`; needs
  decryption + a binary map/database parser (cf. WolfTrans/WolfDec).
- **ADRIFT / RAGS** — old, niche adventure engines with obfuscated binary game
  files; low return.
- **Flash** — `.swf` compiled ActionScript; text is embedded and needs SWF
  decompilation. Flash is end-of-life; low priority.

### Out of scope (different paradigm)
- **Unity** — text is compiled into assemblies (IL2CPP native or Mono DLL) and
  varied asset types (TextMeshPro, TextAsset, `resources.assets`). The established
  approach is a **runtime translation hook** (XUnity.AutoTranslator), not static
  file extraction — a fundamentally different model from this tool.
- **WebGL** — not an engine; a build target (usually Unity WebGL or an HTML5
  export). Treat as Unity or HTML depending on the source.

## How this maps to our roadmap
The engine-adding pattern (implement `GameEngine`, byte-span locator, `mask_for`
+ `codes.ts`, skip derived files, fixture + round-trip test) is documented in
`ROADMAP.md`. Text-based engines came first (Ren'Py → TyranoScript → KiriKiri).
Next up the same **text-based** track: **Godot** (`.po`/`.csv`) or **HTML**
(Twine/SugarCube) — after which decide whether the **VX Ace** audience justifies
a Ruby Marshal codec. Unity/Flash are out of scope for a file-extract tool.
