"""Bake target-language (Thai) glyphs into EVERY TMP_FontAsset of a Unity game —
readable-typetree fonts (edited directly) and stripped-typetree fonts (raw-blob
transplant). Generalizes bake_font_into_stripped_tmp.py to the whole game; this is the
reference for the app's `embed_font` SDF-bake path (unity-textbl).

Usage: python bake_all_fonts.py <game_root> <font.ttf> [uni_start uni_end]
Discovers a font's atlas/material/source-font by TMP's naming convention
("<base> SDF" -> "<base> Atlas", "<base> Atlas Material", "<base>"), so a stripped
font's PPtrs can be re-pointed at its own file's objects.
"""
import sys, os, io, glob, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import UnityPy, numpy as np, freetype
from sdf_lib import sdf_glyph
from PIL import Image

ROOT = sys.argv[1]
TTF = sys.argv[2]
U0 = int(sys.argv[3], 0) if len(sys.argv) > 3 else 0x0E00
U1 = int(sys.argv[4], 0) if len(sys.argv) > 4 else 0x0E7F
PACK_X0 = 590                     # free atlas band (right of the Latin block)
log = io.StringIO()


def game_files(root):
    data = next(glob.iglob(os.path.join(root, "*_Data")), None) or root
    out = glob.glob(os.path.join(data, "*.assets"))
    out += glob.glob(os.path.join(data, "StreamingAssets", "aa", "StandaloneWindows64", "*.bundle"))
    return [p for p in sorted(set(out)) if os.path.isfile(p)]


def script_class(o):
    try:
        return o.read().m_Script.read().m_ClassName
    except Exception:
        return ""


def render_thai(face):
    """(cp, alpha, metrics, w, h) for every target glyph the font covers."""
    out = []
    for cp in range(U0, U1 + 1):
        if face.get_char_index(cp) == 0:
            continue
        a, m, (w, h) = sdf_glyph(face, cp)
        out.append((cp, a, m, w, h))
    return out


def bake_atlas(tex, thai):
    """Composite Thai SDF into a TMP atlas Texture2D (in free band X>=PACK_X0).
    Returns {cp: (rect, metrics)} for the glyphs placed."""
    img = np.array(tex.image)
    H, W = img.shape[:2]
    alpha = img[..., 3].copy()
    placed = {}
    cx, cy, rowh = PACK_X0, 2, 0
    for cp, a, m, w, h in sorted([g for g in thai if g[1] is not None], key=lambda g: -g[4]):
        if cx + w + 4 > W:
            cx = PACK_X0; cy += rowh + 4; rowh = 0
        if cy + h + 4 > H:
            break
        alpha[cy:cy + h, cx:cx + w] = a
        placed[cp] = ({"m_X": cx, "m_Y": H - (cy + h), "m_Width": w, "m_Height": h}, m)
        cx += w + 4; rowh = max(rowh, h)
    img[..., 3] = alpha
    tex.image = Image.fromarray(img)
    tex.save()
    return placed


def extend_tables(tmp, placed):
    """Append Thai glyph + character entries to a font typetree in place (static)."""
    next_idx = max((g["m_Index"] for g in tmp["m_GlyphTable"]), default=0) + 1
    have = {c["m_Unicode"] for c in tmp["m_CharacterTable"]}
    for cp, (rect, m) in placed.items():
        if cp in have:
            continue
        gi = next_idx; next_idx += 1
        tmp["m_GlyphTable"].append({
            "m_Index": gi,
            "m_Metrics": {"m_Width": m["width"], "m_Height": m["height"],
                          "m_HorizontalBearingX": m["bearingX"], "m_HorizontalBearingY": m["bearingY"],
                          "m_HorizontalAdvance": m["advance"]},
            "m_GlyphRect": rect, "m_Scale": 1.0, "m_AtlasIndex": 0, "m_ClassDefinitionType": 0})
        tmp["m_CharacterTable"].append({"m_ElementType": 1, "m_Unicode": cp, "m_GlyphIndex": gi, "m_Scale": 1.0})
    tmp["m_AtlasPopulationMode"] = 0


def find_by_name(objs, type_name, name):
    for o in objs:
        if o.type.name != type_name:
            continue
        try:
            if o.read().m_Name == name:
                return o.path_id
        except Exception:
            pass
    return None


def main():
    face = freetype.Face(TTF)
    thai = render_thai(face)
    log.write(f"target glyphs: {len(thai)} (U+{U0:04X}..U+{U1:04X})\n")

    files = game_files(ROOT)
    # pass 1: donor typetrees for stripped fonts, keyed by font m_Name
    donors = {}
    for p in files:
        try: env = UnityPy.load(p)
        except Exception: continue
        for o in env.objects:
            if o.type.name != "MonoBehaviour" or script_class(o) != "TMP_FontAsset":
                continue
            try:
                t = o.read_typetree()
            except Exception:
                continue
            donors.setdefault(t["m_Name"], t)
    log.write(f"donor font typetrees: {list(donors)}\n")

    # pass 2: bake into every font of every file
    for p in files:
        try: env = UnityPy.load(p)
        except Exception: continue
        objs = list(env.objects)
        changed = False
        for o in objs:
            if o.type.name != "MonoBehaviour" or script_class(o) != "TMP_FontAsset":
                continue
            raw = o.get_raw_data()
            # font name from blob header (works for stripped too): [gameObj12][enabled4][script12][name]
            nlen = struct.unpack_from("<i", raw, 28)[0]
            fname = raw[32:32 + nlen].decode("utf-8", "replace")
            base = fname[:-4] if fname.endswith(" SDF") else fname
            atlas_pid = find_by_name(objs, "Texture2D", base + " Atlas")
            if atlas_pid is None:
                log.write(f"  {os.path.basename(p)}: {fname!r} no atlas — skip\n"); continue
            tex = next(x for x in objs if x.path_id == atlas_pid).read()
            placed = bake_atlas(tex, thai)

            readable = True
            try:
                t = o.read_typetree()
            except Exception:
                readable = False
            if readable:
                extend_tables(t, placed)
                o.save_typetree(t)
            else:
                donor = donors.get(fname)
                if donor is None:
                    log.write(f"  {os.path.basename(p)}: stripped {fname!r} no donor — skip\n"); continue
                import copy
                dt = copy.deepcopy(donor)
                extend_tables(dt, placed)
                # re-point PPtrs at THIS file's objects (by name convention)
                mat = find_by_name(objs, "Material", base + " Atlas Material")
                src = find_by_name(objs, "Font", base)
                dt["m_Script"] = {"m_FileID": struct.unpack_from("<i", raw, 16)[0],
                                  "m_PathID": struct.unpack_from("<q", raw, 20)[0]}
                dt["m_AtlasTextures"] = [{"m_FileID": 0, "m_PathID": atlas_pid}]
                if mat is not None: dt["material"] = {"m_FileID": 0, "m_PathID": mat}
                if src is not None: dt["m_SourceFontFile"] = {"m_FileID": 0, "m_PathID": src}
                # serialize via a donor-owning bundle object, round-trip to get new bytes
                blob = serialize_via_donor(files, fname, dt)
                o.set_raw_data(blob)
            changed = True
            log.write(f"  {os.path.basename(p)}: baked {len(placed)} into {fname!r} ({'typetree' if readable else 'transplant'})\n")
        if changed:
            out = p + ".new"
            open(out, "wb").write(env.file.save(packer="none" if p.endswith(".assets") else "lz4"))
            log.write(f"  wrote {os.path.basename(out)}\n")
    open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "bake_all_fonts.log"), "w", encoding="utf-8").write(log.getvalue())
    print("done — review .log, then move .new files over the originals")


_donor_cache = {}
def serialize_via_donor(files, fname, tree):
    """Write `tree` through a bundle object that owns a readable copy of this font, then
    round-trip the bundle so get_raw_data returns the serialized bytes."""
    for p in files:
        if not p.endswith(".bundle"):
            continue
        env = UnityPy.load(p)
        for o in env.objects:
            if o.type.name != "MonoBehaviour" or script_class(o) != "TMP_FontAsset":
                continue
            try: t = o.read_typetree()
            except Exception: continue
            if t.get("m_Name") != fname:
                continue
            o.save_typetree(tree)
            tmpb = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_reser.bundle")
            open(tmpb, "wb").write(env.file.save(packer="none"))
            e2 = UnityPy.load(tmpb)
            for o2 in e2.objects:
                if o2.type.name == "MonoBehaviour" and script_class(o2) == "TMP_FontAsset":
                    try: t2 = o2.read_typetree()
                    except Exception: continue
                    if t2.get("m_Name") == fname:
                        return o2.get_raw_data()
    raise RuntimeError(f"no donor bundle for {fname}")


if __name__ == "__main__":
    main()
