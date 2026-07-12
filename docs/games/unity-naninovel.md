---
title: Unity — Naninovel (managed text + dialogue)
aliases:
  - Unity Naninovel
  - unity engine
tags:
  - type/research
  - engine/unity
  - game/my-milf-stepmom
created: 2026-07-12
status: implemented
---

# Unity — Naninovel (managed text + dialogue)

Research + engine note for the `unity` engine: Unity **Naninovel** games. It
translates two text layers — **managed text** (UI / names / gallery, in built-in
`TextAsset`s) and the **compiled story dialogue** (raw-spliced out of the
`Naninovel.Script` MonoBehaviours). Part of [[games]]; see [[ENGINES]] and
[[ROADMAP]].

## The feasibility split (why Unity is per-game, not one answer)

Two real games bracket the range:

- **My MILF Stepmom** — Unity **Mono** + **Naninovel**. Two text layers, **both now
  handled**:
  1. **Managed text** in `resources.assets` (~187 usable strings/locale after
     skipping the `Locales` language-name list: UI, character names, gallery
     titles/descriptions, scripted-UI text) — the built-in **`TextAsset` class**, no
     game DLL / typetree needed. 🟢
  2. **Story dialogue** — compiled into `Naninovel.Script` MonoBehaviours with
     **stripped typetrees** and `[SerializeReference]` polymorphic script-lines
     UnityPy can't read structurally. But the spoken text is plain length-prefixed
     UTF-8 in the raw blob, so the engine enumerates + splices it directly on the
     bytes (no typetree). 🟢
  The game was built with Naninovel's `translate` localization: per script it ships
  a **source MB (Chinese)** plus a **`zh-CN → en`** and a **`zh-CN → zh-HK`**
  localization MB (each embeds a `… to English <en> localization document for
  \`Script\`` header). So the *English* story text is already in the file — the
  engine can extract it and translate **English → Thai**, which is what a Thai
  translator wants (read the English, not the Chinese).
- **MR 6.2 / Milf's Resort** — Unity **Mono**, custom dialogue, **6 893
  MonoBehaviours, 0 typetree-readable** (typetrees stripped), *not* Naninovel. Text
  is locked in custom serialization needing type info reconstructed from
  `Assembly-CSharp.dll`. 🔴 **out of scope** (XUnity.AutoTranslator's runtime-hook
  territory, not this app's file-rewrite model). Declined at detection (no Naninovel
  assembly).

## How it works

Unity `.assets` are binary, so — like the AnvilNext engines feed on an external tool
— the binary work is delegated to a **UnityPy helper**
(`src-tauri/resources/unity/rpgtl_unity.py`), driven from `engine/unity.rs` the way
[[../CLAUDE|Ren'Py]] drives the vendored unrpyc decompiler.

Unity games ship no Python, so the release build **embeds a frozen interpreter**:
`scripts/freeze-unity-sidecar.ps1` runs PyInstaller to build `rpgtl-unity.exe` (a
one-file exe, ~66 MB; UnityPy's texture deps are excluded and stubbed since only text
is touched), `build.rs` `include_bytes!`s it into the Rust binary, and the engine
materializes + runs it — no system dependency. The exe is a git-ignored build
artifact (regenerate before a release build); when it is absent (a plain `cargo
build`, CI, or a non-Windows host) the engine falls back to the **system `python`** +
the plain script and degrades with an actionable "install UnityPy" error.

- **Detection** — a `<name>_Data/` dir with `resources.assets` **and** a Naninovel
  runtime assembly (`*Naninovel*.dll`) in `Managed/`. Plain Unity games lack the
  assembly and are declined.
- **Tier 1 — managed text.** UnityPy reads/edits a `TextAsset.m_Script` (Naninovel
  `Key: Value` docs) and re-serializes the `SerializedFile`. Pointer:
  `"<file>#<pathId>#<key>"`.
- **Tier 2 — dialogue (raw byte-splice).** A script MB is fingerprinted by the
  `ScriptLine` type name its SerializeReference table embeds. Strings are enumerated
  at 4-byte-aligned offsets (`[i32 len][utf8][pad 4]`) — aligned scanning is precise
  and cannot desync — then filtered to spoken text (drop paths / commands / ids /
  punctuation-only, split off a `Char:` author prefix). Each line is addressed by its
  **index** in that deterministic enumeration, so export and import agree without
  storing byte offsets (which shift when a different-length translation is spliced).
  Pointer: `"dlg#<file>#<pathId>#<idx>"`. Inject rewrites the string's length prefix +
  4-byte alignment and re-attaches the author prefix; `env.file.save()` rebuilds the
  file-level object-size table (serialized data is inline, no internal byte pointers).
- **Locale slot** — for **both** tiers the engine targets the **`en`** localization
  (default): managed text picks the `to English` docs, and dialogue picks the
  `zh-CN → en` localization MBs (keeping the non-CJK translated lines, skipping the
  Chinese source references the loc doc carries beside them). The player selects
  English in-game and sees the translation; other locales stay intact. A game with no
  localization falls back to its source docs / source scripts (base language becomes
  the translation).
- **Round-trip** — **load-faithful, not byte-identical** (a documented exception like
  KiriKiri's UTF-16 fallback): UnityPy re-serializes the whole file. The helper emits
  **only the files it changed**; untouched `.assets` are byte-copied. `inject` runs
  the helper into a private temp dir then relocates the changed files into `out_dir`,
  because UnityPy holds the source open while saving (same-dir write is a Windows
  sharing violation).
- **Masking** — `protect::mask_unity`: TMPro rich-text tags (`<color=…>`, `<b>`,
  `<br>`), `{0}`/`{name}` format args, `\n`. `[…]` and `%` stay visible. Mirrored in
  `src/codes.ts` + `src/messageWidth.ts`.

## Validation (all on My MILF Stepmom, 2026-07-12)

- Managed text: export→patch→import round-trip byte-clean; **in-game** the `en` UI
  docs patched with a `[TH]` marker showed on the English main menu.
- Dialogue mechanism: a marker appended to ~1900 raw dialogue strings rendered
  **on-screen** in the compiled story — proving the raw-splice reaches the displayed
  text despite the stripped typetree.
- **EN → TH, end to end**: extracted the 523 English dialogue lines from the
  `zh-CN → en` localization MBs, hand-translated the opening to Thai, injected, and
  **Thai rendered correctly in-game** in English mode (e.g. `I feel a sense of peace
  ◆ไทยTH◆`). The game's TMPro font resolves Thai through its fallback table — **no
  tofu**, so no font embedding is needed for this game.
- Rust engine e2e (`tests/unity_roundtrip.rs`, opt-in via `RPGTL_UNITY_GAME`):
  extract (both tiers) → mark every unit → inject (TextAsset patch + dialogue splice)
  → re-extract → a Dialogue-kind marker survives. Full suite 192 pass, warning-free.

## Known gaps / next

- **Dialogue enumeration is heuristic.** Spoken text is separated from command args /
  asset paths / ids by content heuristics, so a few non-dialogue lines can still leak
  into the grid (and the odd real line be missed). Inject is safe regardless — only
  translated units are spliced, and each idx is matched deterministically. A
  structure-aware parse of the compiled ScriptLine layout would tighten it.
- **Thai shaping** — simple Thai renders (the game's TMPro fallback font has Thai
  glyphs). Games whose font lacks Thai would need a TMP SDF font embedded (materially
  harder than the Ren'Py/RPGMaker TTF swap); not needed for this game, deferred until
  one requires it. Complex stacked tone/vowel marks are unverified.
- **Non-`en` targets** — the dialogue locale filter keeps non-CJK (Latin) translated
  lines, so a `zh → zh-HK` (CJK→CJK) localization can't be separated from its source
  by script alone; `en` and other Latin targets work.
- **Tier 3** — generic `TextAsset` text (plain `.txt`/`.csv`/`.json`, I2 Localization
  CSV-in-TextAsset) for non-Naninovel Unity games, with content heuristics.

## See also

- [[games]] — game-research index
- [[ENGINES]] — engine translatability reference
- [[ROADMAP]] — next engines + engine-adding pattern
- [[anvilnext-locpackage-format]] — the other "engine fed by an external tool" pattern
