import sys, io
sys.path.insert(0, r"C:\Users\Mac\AppData\Local\Temp\claude")
import UnityPy, numpy as np, freetype
from sdf_lib import sdf_glyph
from PIL import Image

import os
BUNDLE = sys.argv[1] if len(sys.argv) > 1 else "s.event_assets_all_d7278a6b6d69cd43412536636515b3ec.bundle"
GAME = r"F:\Downloads\otomi-games.com_RKI5ZOR34\NTR Soccer"
SRC = os.path.join(GAME, r".rpgtl\source\NTR_Soccer_Data\StreamingAssets\aa\StandaloneWindows64", BUNDLE)
LIVE = os.path.join(GAME, r"NTR_Soccer_Data\StreamingAssets\aa\StandaloneWindows64", BUNDLE)
SARABUN = r"C:\Users\Mac\Works\translator-game\src-tauri\resources\Sarabun-Regular.ttf"
FONT_NAME_MATCH = sys.argv[2] if len(sys.argv) > 2 else "Playpen"
log = io.StringIO()

face = freetype.Face(SARABUN)
# Thai codepoints Sarabun covers (0E00-0E7F) with a real glyph
thai = [cp for cp in range(0x0E00, 0x0E80) if face.get_char_index(cp) != 0]
log.write(f"Thai codepoints in Sarabun: {len(thai)}\n")

env = UnityPy.load(SRC)
tmp_obj = None; tmp = None; apid = None
for o in env.objects:
    if o.type.name != "MonoBehaviour": continue
    try: t = o.read_typetree()
    except: continue
    if isinstance(t, dict) and t.get("m_AtlasPopulationMode") == 1 and FONT_NAME_MATCH in t.get("m_Name", ""):
        tmp_obj, tmp = o, t
        apid = (t.get("m_AtlasTextures") or [{}])[0].get("m_PathID")
        break
log.write(f"TMP font: {tmp.get('m_Name')} glyphs={len(tmp['m_GlyphTable'])} chars={len(tmp['m_CharacterTable'])}\n")

# atlas image (RGBA-decoded; SDF lives in alpha). Work top-down; convert to Unity Y later.
tex_obj = next(o for o in env.objects if o.type.name == "Texture2D" and o.path_id == apid)
tex = tex_obj.read()
img = np.array(tex.image)                 # H,W,4  (top-down)
H, W = img.shape[:2]
alpha = img[..., 3].copy()

# render+SDF all Thai glyphs
glyphs = []   # (cp, alpha_bitmap, metrics, w, h)
for cp in thai:
    a, m, (w, h) = sdf_glyph(face, cp)
    if a is None or w == 0 or h == 0:
        # zero-width (rare) — still register a char with advance, no rect
        glyphs.append((cp, None, m, 0, 0)); continue
    glyphs.append((cp, a, m, w, h))

# shelf-pack into the free right band: X0..W, Y 0..H (top-down)
X0 = 590; GAP = 4
placed = {}   # cp -> (ax, ay)  top-left in top-down coords
cx, cy, rowh = X0, 2, 0
for cp, a, m, w, h in sorted([g for g in glyphs if g[1] is not None], key=lambda g: -g[4]):
    if cx + w + GAP > W:
        cx = X0; cy += rowh + GAP; rowh = 0
    if cy + h + GAP > H:
        log.write(f"OUT OF ATLAS SPACE at cp {cp:04x}\n"); break
    alpha[cy:cy + h, cx:cx + w] = a
    placed[cp] = (cx, cy)
    cx += w + GAP; rowh = max(rowh, h)
log.write(f"packed {len(placed)} Thai glyphs; last row y={cy}\n")

# write alpha back into the RGBA image, set_image
img[..., 3] = alpha
tex.image = Image.fromarray(img)
tex.save()

# build new glyph/char entries (keep existing Latin)
next_idx = max(g["m_Index"] for g in tmp["m_GlyphTable"]) + 1
new_glyphs = list(tmp["m_GlyphTable"])
new_chars = list(tmp["m_CharacterTable"])
existing_chars = {c["m_Unicode"] for c in new_chars}
added = 0
for cp, a, m, w, h in glyphs:
    if cp in existing_chars: continue
    gi = next_idx; next_idx += 1
    if a is not None and cp in placed:
        ax, ay = placed[cp]
        uy = H - (ay + h)                # Unity bottom-up Y
        rect = {"m_X": ax, "m_Y": uy, "m_Width": w, "m_Height": h}
    else:
        rect = {"m_X": 0, "m_Y": 0, "m_Width": 0, "m_Height": 0}
    new_glyphs.append({
        "m_Index": gi,
        "m_Metrics": {"m_Width": m["width"], "m_Height": m["height"],
                      "m_HorizontalBearingX": m["bearingX"], "m_HorizontalBearingY": m["bearingY"],
                      "m_HorizontalAdvance": m["advance"]},
        "m_GlyphRect": rect, "m_Scale": 1.0, "m_AtlasIndex": 0, "m_ClassDefinitionType": 0,
    })
    new_chars.append({"m_ElementType": 1, "m_Unicode": cp, "m_GlyphIndex": gi, "m_Scale": 1.0})
    added += 1
tmp["m_GlyphTable"] = new_glyphs
tmp["m_CharacterTable"] = new_chars
tmp["m_AtlasPopulationMode"] = 0            # Static: TMP uses the baked tables/atlas as-is
tmp_obj.save_typetree(tmp)
log.write(f"added {added} Thai chars -> glyphs={len(new_glyphs)} chars={len(new_chars)}\n")

# repack -> LIVE
blob = None
for packer in ("lz4", "none"):
    try: blob = env.file.save(packer=packer); break
    except Exception as e: log.write(f"packer {packer} failed: {e}\n")
open(LIVE, "wb").write(blob)
log.write(f"wrote LIVE s.event ({len(blob)} bytes)\n")
open(r"C:\Users\Mac\AppData\Local\Temp\claude\bake_thai.txt", "w", encoding="utf-8").write(log.getvalue())
print("done")
