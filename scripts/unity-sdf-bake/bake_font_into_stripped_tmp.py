import sys, io, os, struct
sys.path.insert(0, r"C:\Users\Mac\AppData\Local\Temp\claude")
import UnityPy, numpy as np, freetype
from sdf_lib import sdf_glyph
from PIL import Image

GAME = r"F:\Downloads\otomi-games.com_RKI5ZOR34\NTR Soccer"
SHARED = os.path.join(GAME, r"NTR_Soccer_Data\sharedassets0.assets")
SRC_BUNDLE = os.path.join(GAME, r".rpgtl\source\NTR_Soccer_Data\StreamingAssets\aa\StandaloneWindows64\characterpose_assets_all_0db51f8333e3230784e238da49432f5a.bundle")
SARABUN = r"C:\Users\Mac\Works\translator-game\src-tauri\resources\Sarabun-Regular.ttf"
log = io.StringIO()

# --- 1) base font typetree from a bundle copy (89 Latin, full structure) ---
benv = UnityPy.load(SRC_BUNDLE)
bfont = None
for o in benv.objects:
    if o.type.name != "MonoBehaviour": continue
    try: t = o.read_typetree()
    except: continue
    if isinstance(t, dict) and t.get("m_AtlasPopulationMode") is not None and "Playpen" in t.get("m_Name",""):
        bfont, tmp = o, t; break
log.write(f"base font: {tmp['m_Name']} glyphs={len(tmp['m_GlyphTable'])} chars={len(tmp['m_CharacterTable'])}\n")

# --- 2) sharedassets0: atlas image (pid 1461) ---
senv = UnityPy.load(SHARED)
sbyid = {o.path_id: o for o in senv.objects}
tex_obj = sbyid[1461]
tex = tex_obj.read()
img = np.array(tex.image); H, W = img.shape[:2]
alpha = img[..., 3].copy()
log.write(f"atlas 1461: {tex.image.mode} {W}x{H}\n")

# --- 3) render+SDF Thai, pack into free right band (X>=590) ---
face = freetype.Face(SARABUN)
thai = [cp for cp in range(0x0E00, 0x0E80) if face.get_char_index(cp) != 0]
glyphs = []
for cp in thai:
    a, m, (w, h) = sdf_glyph(face, cp)
    glyphs.append((cp, a, m, w, h))
X0 = 590; GAP = 4
placed = {}; cx, cy, rowh = X0, 2, 0
for cp, a, m, w, h in sorted([g for g in glyphs if g[1] is not None], key=lambda g: -g[4]):
    if cx + w + GAP > W: cx = X0; cy += rowh + GAP; rowh = 0
    if cy + h + GAP > H: log.write("OUT OF SPACE\n"); break
    alpha[cy:cy+h, cx:cx+w] = a; placed[cp] = (cx, cy); cx += w + GAP; rowh = max(rowh, h)
log.write(f"packed {len(placed)} Thai into atlas 1461 (last y={cy})\n")
img[..., 3] = alpha
tex.image = Image.fromarray(img); tex.save()

# --- 4) build Thai glyph/char entries + append to the base tables ---
next_idx = max(g["m_Index"] for g in tmp["m_GlyphTable"]) + 1
ng = list(tmp["m_GlyphTable"]); nc = list(tmp["m_CharacterTable"])
existing = {c["m_Unicode"] for c in nc}
added = 0
for cp, a, m, w, h in glyphs:
    if cp in existing: continue
    gi = next_idx; next_idx += 1
    if a is not None and cp in placed:
        ax, ay = placed[cp]; rect = {"m_X": ax, "m_Y": H-(ay+h), "m_Width": w, "m_Height": h}
    else:
        rect = {"m_X": 0, "m_Y": 0, "m_Width": 0, "m_Height": 0}
    ng.append({"m_Index": gi, "m_Metrics": {"m_Width": m["width"], "m_Height": m["height"],
        "m_HorizontalBearingX": m["bearingX"], "m_HorizontalBearingY": m["bearingY"],
        "m_HorizontalAdvance": m["advance"]}, "m_GlyphRect": rect, "m_Scale": 1.0,
        "m_AtlasIndex": 0, "m_ClassDefinitionType": 0})
    nc.append({"m_ElementType": 1, "m_Unicode": cp, "m_GlyphIndex": gi, "m_Scale": 1.0})
    added += 1
tmp["m_GlyphTable"] = ng; tmp["m_CharacterTable"] = nc
tmp["m_AtlasPopulationMode"] = 0                        # static
# --- fix PPtrs to sharedassets0's objects ---
tmp["m_Script"] = {"m_FileID": 1, "m_PathID": 3171}
tmp["material"] = {"m_FileID": 0, "m_PathID": 6}
tmp["m_SourceFontFile"] = {"m_FileID": 0, "m_PathID": 2211}
tmp["m_AtlasTextures"] = [{"m_FileID": 0, "m_PathID": 1461}]
log.write(f"font tables -> glyphs={len(ng)} chars={len(nc)} (+{added} Thai)\n")

# --- 5) serialize via the bundle typetree: save_typetree, then round-trip the
#        bundle through save+reload so get_raw_data returns the NEW bytes ---
bfont.save_typetree(tmp)
btmp = r"C:\Users\Mac\AppData\Local\Temp\claude\_bundle_reser.bundle"
open(btmp, "wb").write(benv.file.save(packer="none"))
benv2 = UnityPy.load(btmp)
blob = None
for o in benv2.objects:
    if o.type.name != "MonoBehaviour": continue
    try: tt = o.read_typetree()
    except: continue
    if isinstance(tt, dict) and tt.get("m_AtlasPopulationMode") is not None and "Playpen" in tt.get("m_Name",""):
        blob = o.get_raw_data()
        log.write(f"re-serialized font: chars={len(tt['m_CharacterTable'])} script={tt.get('m_Script')} atlas={tt.get('m_AtlasTextures')}\n")
        break
log.write(f"serialized font blob: {len(blob)} bytes (was 9588)\n")

# --- 6) transplant into sharedassets0 pid=3527 ---
sbyid[3527].set_raw_data(blob)

# --- 7) write sharedassets0 (temp then replace; preserves DS dialogue etc.) ---
out_bytes = senv.file.save()
tmp_path = SHARED + ".new"
open(tmp_path, "wb").write(out_bytes)
os.replace(tmp_path, SHARED)
log.write(f"wrote sharedassets0 ({len(out_bytes)} bytes)\n")
open(r"C:\Users\Mac\AppData\Local\Temp\claude\bake_shared.txt", "w", encoding="utf-8").write(log.getvalue())
print("done")
