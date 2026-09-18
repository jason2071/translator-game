---
title: "Ren'Py — Thai text renders as tofu squares when the game's fonts are FontGroup objects"
tags:
  - type/case
  - engine/renpy
created: 2026-09-18
status: repo fix implemented 2026-09-18 (unreleased) — hook wraps FontGroup fonts + whole-face map gated to pre-8.1, regression tests in `renpy.rs` unit tests
---

# Ren'Py — Thai text renders as tofu squares when the game's fonts are FontGroup objects

**Game:** City Lights & Love Bites — Season 1 (compiled-only Ren'Py 8.5.2, Thai
export via `tl/thai/`, source language = the game's own `tl/Chinese`). Third
case on this game in one afternoon, after
[[2026-09-18-renpy-empty-screen-decompile]] and
[[2026-09-18-renpy-redecompile-clobber]].

**User-visible failure** — after translating and exporting to the game, every
Thai string rendered as empty squares (tofu). User's screenshot: the replay
menu (top-right of the screen, entries like 第一次进课堂 now Thai) was rows of
boxes. The same menu in Chinese renders fine. No error log — the game boots
and runs; this is a silent rendering failure.

## Root cause

1. **The game's every main text style is a FontGroup *object*, not a font
   name.** `gui.rpy`:

   ```renpy
   cllb_mixed_font = renpy.text.font.FontGroup()
   cllb_mixed_font.add("fonts/SourceHanSansCN-Regular.ttf", 0x2E80, 0xA4CF)  # CJK
   cllb_mixed_font.add("gui/fonts/Nunito-Bold.ttf", None, None)              # default
   define gui.text_font = cllb_mixed_font           # also name/interface/button/choice
   ```

2. **Our `rpgtl_thai` font transform deliberately skipped non-string fonts:**

   ```python
   def _tl_font_group(_f):
       if not isinstance(_f, str):
           return _f        # ← the game's FontGroup passes through unwrapped
   ```

   So the group never gained a Thai slot: Thai codepoints (U+0E00–U+0E7F) fall
   to the group's `None` default — Nunito — which has no Thai glyphs → tofu.

3. **`config.font_replacement_map` cannot rescue it** — its keys are font
   *names*; a style whose font is an object never matches a key.

4. Verified against the game's shipped Ren'Py 8.5.2 source:
   `renpy/text/text.py:447-451` calls the transform with the segment's font
   whatever it is (string or FontGroup), and `FontGroup.add(inner, None, None)`
   (`renpy/text/font.py:890-899`) *flattens* a nested group's ranges with
   first-add precedence — so wrapping is the officially supported route.

5. **Second layer (found after the first fix shipped and boxes persisted):**
   the same export also populated `config.font_replacement_map` for every game
   font *unconditionally*. That map is a whole-face swap applied in
   `get_font` (`font.py:712`) — **after** the FontGroup has already picked the
   right face per glyph. So untranslated CJK leftovers (the six replay
   choices, logged in Chinese: `季雅 - 女仆小手`) resolved SourceHanSans via
   the group, then got swapped to Sarabun (no CJK) → boxes. Latin (`[Yua]`)
   in the same labels rendered fine, which is what made the screenshots look
   like "partial Thai coverage".

## Game-side fix (2026-09-18 ~16:31)

None needed — the user restored the game to clean before reporting (the
`.rpgtl/` sidecar including `project.db` was removed with it, so the
translation work was re-done after the repo fix). The repaired export comes
from re-exporting with the fixed build below.

## Repo fix (implemented 2026-09-18, unreleased)

`setup_language_with_runtime_strings` now emits a hook that wraps **strings
and FontGroups**:

```python
def _tl_font_group(_f):
    if not isinstance(_f, (str, FontGroup)):
        return _f
    _g = _tl_groups.get(_f)
    if _g is None:
        try:
            _g = FontGroup().add(_tl_font, 0x0e00, 0x0e7f).add(_f, None, None)
            if hasattr(_f, "char_map"):
                for _k, _v in _f.char_map.items():
                    if _k not in _g.char_map:
                        _g.char_map[_k] = _v
        except Exception:
            _g = _f
        _tl_groups[_f] = _g
    return _g
```

- `FontGroup.add(_f, None, None)` flattens the game's group into ours; because
  the Thai range was added first, Thai stays on the bundled Sarabun face while
  CJK keeps SourceHanSans and Latin keeps the game's own face.
- `add()` copies `map` only, so the hook merges the inner group's `char_map`
  (`.remap()` data) by hand — without it, a remapping game would silently lose
  remaps when wrapped.
- Anything that is neither a string nor a FontGroup still passes through
  unchanged (the old behavior for exotic font objects).

Regression test: `setup_language_hook_wraps_fontgroup_fonts` asserts the
emitted hook accepts FontGroups and merges char_map. Behavioral proof: ran the
new hook against the real `FontGroup` class extracted from the game's shipped
`renpy/text/font.py` (bundled python) — Thai→tl_font, CJK→SourceHan,
Latin→Nunito, remap preserved, cache stable, string fonts wrap as before
(10/10).

**Layer-2 fix:** the whole-face `font_replacement_map` loop is now emitted
under `else:` of `if hasattr(config, "font_transforms")` — it only runs on
Ren'Py < 8.1 where the non-destructive transform cannot exist. On 8.1+ the
transform alone covers string fonts *and* FontGroups, and untranslated
leftovers keep rendering in the game's own faces instead of being swapped to
a face that lacks their glyphs. Regression test:
`setup_language_maps_whole_faces_only_without_font_transforms`;
`setup_language_installs_a_glyph_level_font_fallback` updated to pin the gate.
Verified live: patched the game's `zzz_translator.rpy` (our overlay file) with
the same gate, booted — recompiled clean, `errors.txt` untouched.

## Follow-up found while closing this case (fixed same day)

The game's replay-menu captions (季雅 - 女仆小手, 第一次进课堂, …) never became
units: they exist **only in the base script** (`menu:` captions the developer
left in Chinese while the dialogue is English), and the auto tl-source merge
kept base-script units of kind `Term` only — base `Choice` units were dropped
("base dialogue/choices stay out"), so captions the tl tree doesn't cover had
no path into the project at all. Interim game-side fix (now superseded):
`tl/thai/zzzz_rpgtl_replay_menus.rpy`, hand-written strings block.

**Repo fix:** `merge_auto_tl_source` keeps uncovered base-script `Choice`
units (coverage = the tree's strings-block `old` keys, newly collected by
`extract_from_tl` — a caption the tree covers must not arrive twice, in two
languages); `extract_rpy` skips captions that are nothing but interpolation
tags (`"[first_text]"` is a template several options flow through);
`export_tl_from_source_with_font_scale` writes applied base captions to
`tl/<lang>/rpgtl_menus.rpy` as a `translate <lang> strings:` block — strings
match *before* interpolation (`substitute()` translates first), which is the
only mechanism that translates captions carrying `[Yua]`-style tags; the
post-interpolation runtime hook never sees the tag form. Regression tests:
`extract_rpy_skips_pure_interpolation_menu_captions`,
`merge_auto_tl_source_keeps_uncovered_base_menu_captions`,
`export_tl_from_source_writes_base_menu_strings_block`.

Known limit: captions interpolated from label-default parameters
("第一次进行"/"第二次回访") are still untranslated by the app — the literal
`"[first_text]"` template is skipped by design; harvesting display-worthy
label defaults is future work.

## Rule

**Violated [[GAME-SAFETY]] rule 5 (validate what we wrote — in the rendering
sense: "export succeeded" meant the files parse, not that a Thai speaker can
read the result).** Two lessons. First, a font pipeline must cover every shape
a style font can take, not just names: Ren'Py fonts are strings *or FontGroup
objects*, and whole-face replacement maps only ever see the strings. Second,
**a whole-face font swap is destructive fallback, not a default**: it silently
deletes every glyph the target face lacks, and the damage only shows on
*untranslated* leftovers — which no parse check or boot test will ever catch.
When a per-glyph mechanism exists (`config.font_transforms`), use it alone and
keep the whole-face map strictly for platforms where the alternative cannot
run. When adding glyph coverage, test against the target engine's real
classes — not against the happy path where every font is a filename.
