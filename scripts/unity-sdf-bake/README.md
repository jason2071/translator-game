# Unity TMP SDF font baking (Thai) — reference tooling

Working reference for baking **Thai glyphs into a Unity TextMeshPro (TMP) SDF font**
whose game (e.g. NTR Soccer, `unity-textbl` engine) uses **pre-baked atlases** and
does **not** rasterize glyphs dynamically at runtime. Proven in-game: Thai dialogue
renders cleanly. Not yet integrated into the app — this is the reference for a future
`embed_font` SDF-bake path.

## Why the simple swap-font (Milf Plaza) approach fails here

`unity-csvloc`'s `swap-font` only swaps the source TTF of a **Dynamic-atlas**
TMP_FontAsset, relying on the runtime to rasterize new glyphs. NTR Soccer's TMP fonts
ship a **pre-baked** glyph/character table + atlas texture, and the runtime does **not**
add glyphs dynamically (verified: Thai stayed "not found in [font] or any potential
fallbacks", `Player.log`). So Thai must be **baked into the atlas + tables offline**.

## The three-copies trap (the hard part)

The dialogue's font `PlaypenSans-VariableFont_wght SDF` exists as **three copies**:

1. `s.event_…bundle` — TMP asset, readable typetree.
2. `characterpose_…bundle` — TMP asset, readable typetree.
3. **`sharedassets0.assets` (pid 3527)** — TMP asset with a **stripped typetree**
   (UnityPy `read_typetree` fails) → **this is the one the subtitle actually uses.**

Editing copies 1–2 (via typetree) changed nothing in-game. The fix targets copy 3,
which can't be edited via typetree → **raw-blob transplant** (below).

## Approach

`sdf_lib.py` — render a glyph from Sarabun with freetype, compute a signed distance
field (scipy `distance_transform_edt`), encode to the game's atlas convention.
Calibrated against NTR's baked Latin:

- `m_PointSize = 90`, atlas 1024², `m_AtlasPadding = 9`, `m_AtlasRenderMode = 4165` (SDFAA).
- SDF encoding: **`alpha = clip(128 + 13·signed_dist_px, 0, 255)`** (edge = 128, +inside;
  slope ≈ 13 measured off a clean baked stem). Glyph rect = tight ink bbox.

`bake_font_into_bundle.py` — bake Thai into a **readable-typetree** TMP font in a bundle
(pack into free atlas space X≥590, append glyph/character entries, set static mode).

`bake_font_into_stripped_tmp.py` — the **real fix**: bake into the **stripped** TMP font
in `sharedassets0.assets`. Because its typetree can't be written, it:
1. bakes the Thai SDF into that file's atlas Texture2D (pid 1461, readable),
2. builds the 176-char font blob from a **bundle copy's** full typetree (append Thai,
   set static, **fix the PPtrs** — `m_Script {FileID:1,PathID:3171}`, `material→6`,
   `m_SourceFontFile→2211`, `m_AtlasTextures→[1461]` — to `sharedassets0`'s objects),
   round-trips the bundle through save+reload so `get_raw_data` returns the **new** bytes
   (a `save_typetree` gotcha: `get_raw_data` otherwise returns the cached original),
3. `set_raw_data` that blob onto pid 3527, and re-serializes `sharedassets0` (the DS
   dialogue and everything else are preserved).

## Deps / caveats

- `pip install freetype-py numpy scipy pillow UnityPy`.
- Paths are hardcoded to the NTR Soccer install — this is a **reference**, not a general
  tool. Productizing means porting `sdf_lib` into the helper and generalizing copy/PPtr
  discovery (and the freetype/scipy weight vs the frozen sidecar is a real tradeoff).
- Only `PlaypenSans` (dialogue) is baked here; other UI fonts (e.g.
  `851tegaki_zatsu_normal_0883 SDF`) would need the same treatment for UI Thai.
