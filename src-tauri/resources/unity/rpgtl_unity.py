#!/usr/bin/env python3
"""Unity (Naninovel) text bridge for the translator app (engine sidecar).

Reads/writes a Unity game's translatable strings in two tiers:

  * **Managed text** (tier 1) — Naninovel UI / character names / gallery strings,
    stored in built-in `TextAsset` objects as `Key: Value` documents (optionally
    headed by a `; <src> to <dst> localization document for \`Name\`` comment). No
    game DLL / typetree needed; UnityPy round-trips them.
  * **Dialogue** (tier 2) — the compiled story lines, stored in `Naninovel.Script`
    MonoBehaviours whose typetrees are *stripped* and whose script-lines are
    `[SerializeReference]` polymorphic objects UnityPy can't read structurally. But
    the spoken text is plain length-prefixed UTF-8 (`[i32 len][utf8][pad to 4]`)
    inside the raw MonoBehaviour blob, so we enumerate + splice those strings
    directly on the bytes — no typetree required. A script MB is fingerprinted by
    the `ScriptLine` type name its SerializeReference table embeds; each translatable
    line is addressed by its *index* in a deterministic enumeration, so export and
    import agree without storing byte offsets (which shift when a translation of a
    different length is spliced in).

Subcommands (driven by `engine/unity.rs`):

  export <data_dir> <manifest.json> [locale]
      Scan the data dir's `.assets`; emit one JSON record per translatable string:
        managed text: {t:"mt",  file, pathId, name, key, source}
        dialogue:     {t:"dlg", file, pathId, idx, char, source}
      (`char` is the Naninovel author prefix — "Caroline" in "Caroline: hi" — kept
      out of `source` so only the spoken text is translated, and re-attached on
      import so the in-game name mapping still resolves.)

  import <data_dir> <patch.json> <out_dir> [locale]
      Read those records back with a `translation`, patch each managed-text
      TextAsset and splice each dialogue string, and write ONLY the changed
      `.assets` into <out_dir> (mirroring the file name). Untouched files are not
      emitted — the caller keeps the originals. Never writes into <data_dir> (the
      caller may pass out_dir == data_dir; UnityPy still holds the source open, so
      writing a distinct dir avoids a Windows sharing violation).

  swap-font <bundle_in> <font.ttf> <bundle_out>
      For the Unity CSV-localization engine (`engine/unity_csv.rs`): in an
      Addressables font bundle, replace the source TTF of every **Dynamic-atlas**
      TMP_FontAsset (`m_AtlasPopulationMode == 1`) with <font.ttf>, then write the
      modified bundle to <bundle_out>. Dynamic mode rasterizes glyphs at runtime from
      that font, so swapping it in a fallback font makes the game render a script the
      baked atlases lack (e.g. Thai) — no SDF atlas baking. Prints the swap count.

`locale` (default "en") selects the managed-text localization docs whose header
target is that locale. Dialogue is enumerated from every script regardless of
locale (a compiled-script game like this one ships no per-locale scripts).
"""
import sys, os, glob, json, re, struct

# The frozen build (PyInstaller, driven by `scripts/freeze-unity-sidecar.ps1`)
# excludes UnityPy's texture dependencies (PIL, numpy, astc_encoder, …) — this
# engine only touches text and never decodes an image, and those libs are ~60 MB
# of the frozen size. But `import UnityPy` eagerly imports its legacy texture
# patches, which `from PIL import Image` at module load. So under a frozen build
# only, register empty stand-ins for the unused texture libs; the patched functions
# are never called, so the stubs are enough for `import UnityPy` to succeed. Under
# system Python (`sys.frozen` unset) nothing is stubbed and the real libraries are
# used.
if getattr(sys, "frozen", False):
    import types

    def _stub(name):
        mod = types.ModuleType(name)
        mod.__path__ = []
        sys.modules.setdefault(name, mod)
        return sys.modules[name]

    _stub("PIL").Image = _stub("PIL.Image")
    for _name in ("numpy", "astc_encoder", "texture2ddecoder", "etcpak"):
        _stub(_name)

import UnityPy

# --- managed text (tier 1) --------------------------------------------------

# Header of a localization doc: "; Chinese (S) <zh-CN> to English <en> localization
# document for `DefaultUI` managed text document".
HEADER_RE = re.compile(r'^;.*?<[^>]+>\s+to\s+.*?<([^>]+)>.*localization document for `([^`]+)`', re.M)
NAME_RE = re.compile(r'for `([^`]+)`')
# A managed-text record line: a dotted/underscored identifier then ": value".
KEY_RE = re.compile(r'^([A-Za-z0-9_.\-]+):\s?(.*)$')

# Naninovel built-in docs that are infrastructure, not game content, so they're
# skipped: `Locales` is the language-picker's own list of ~233 locale display names
# (Afrikaans, Arabic, …) — translating them floods the grid with noise.
SKIP_DOCS = {"Locales"}

# --- dialogue (tier 2) ------------------------------------------------------

# A script MB embeds these SerializeReference type names in its blob; the shared
# suffix fingerprints "this MonoBehaviour is a compiled Naninovel script".
SCRIPT_FINGERPRINT = b"ScriptLine"
# A game built with Naninovel's `translate` localization ships, per script, a source
# MB plus one localization MB per target locale, whose blob embeds a header like
# "... to English <en> localization document for `Miya_Story_1` naninovel script".
# Group 1 is the target locale code (en, zh-HK, …).
SCRIPT_LOC_HDR = re.compile(rb'to [^<]*<([^>]+)> localization document for `[^`]+`')
# CJK range — used to tell a localization's translated lines (e.g. English) from the
# source-language reference lines a Naninovel localization doc keeps beside them.
CJK_RE = re.compile('[㐀-鿿豈-﫿]')
# Reject non-dialogue strings the raw scan also turns up.
ASCII_PATH = re.compile(r'^[A-Za-z0-9_/.\-]+$')          # pure ASCII id / asset path
KV_ARG = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*=')          # command param "key=value"
DLG_SKIP_PFX = ('@', '#', ';')                            # command / label / comment
DLG_TEXTSIG = re.compile(r'[ 　.,!?…。，！？、:："“”\'()]')   # prose signal
DLG_LETTER = re.compile(r'[A-Za-z0-9㐀-鿿]')                # has something to translate
# Naninovel generic-text line: an ASCII author id, then ": ", then the spoken text.
CHARID_RE = re.compile(r'^([A-Za-z_][A-Za-z0-9_]*):\s(.+)$', re.S)


def enum_strings(raw):
    """Every Unity-serialized string in a blob, as (pos, byte_len, text).

    Unity writes a string as `[i32 length][utf8 bytes][align to 4]`, and a field's
    length prefix always lands on a 4-byte-aligned offset — so scanning only aligned
    offsets is both precise (few false hits) and impossible to desync. Overlapping
    candidates are resolved greedily left-to-right (earliest, then longest), which
    is deterministic, so export and import enumerate identically."""
    n = len(raw)
    cand = []
    for p in range(0, n - 4, 4):
        L = struct.unpack_from("<i", raw, p)[0]
        if 1 <= L <= 8192 and p + 4 + L <= n:
            chunk = raw[p + 4:p + 4 + L]
            if not any(b < 9 for b in chunk):   # real strings carry no NUL/control
                try:
                    cand.append((p, L, chunk.decode("utf-8")))
                except UnicodeDecodeError:
                    pass
    cand.sort(key=lambda c: (c[0], -c[1]))
    chosen, end = [], -1
    for p, L, t in cand:
        if p < end:
            continue
        chosen.append((p, L, t))
        end = p + 4 + L + ((-L) % 4)
    return chosen


def is_dialogue(t):
    """True if a raw string looks like spoken text, not a path / command / id."""
    if len(t) < 2 or t.startswith(DLG_SKIP_PFX):
        return False
    if ASCII_PATH.match(t) or KV_ARG.match(t) or '/' in t:
        return False
    if 'localization document' in t:        # a Naninovel loc-doc header, not a line
        return False
    if not DLG_LETTER.search(t):            # punctuation / ellipsis only — nothing to translate
        return False
    return bool(DLG_TEXTSIG.search(t)) or len(t) >= 3


def dialogue_units(raw, localized):
    """Ordered [(pos, byte_len, char_or_None, text)] of translatable dialogue in a
    script MB. The list order defines the stable per-MB `idx` used as the pointer.

    A localization MB (`localized=True`) also carries the source-language reference
    lines beside each translation, so skip the source-script (CJK) strings and keep
    only the translated text — that is what the player sees for this locale and what
    should be re-translated. A source MB (`localized=False`) keeps everything."""
    units = []
    for p, L, t in enum_strings(raw):
        if not is_dialogue(t):
            continue
        m = CHARID_RE.match(t)
        char, text = (m.group(1), m.group(2)) if m else (None, t)
        if not is_dialogue(text):
            continue
        if localized and CJK_RE.search(text):
            continue
        units.append((p, L, char, text))
    return units


def script_mbs(env, locale):
    """The compiled script MBs to translate for `locale`, and whether they are
    localization docs.

    Prefer the localization MBs whose embedded header targets `locale` — they hold
    the text the player sees in that language, so translating them (e.g. English →
    Thai) works from a language the translator reads. Fall back to the source MBs
    when the game ships no localization for `locale` (translating the base
    language). Returns ([(obj, raw), …], localized)."""
    loc, src = [], []
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            raw = obj.get_raw_data()
        except Exception:
            continue
        if SCRIPT_FINGERPRINT not in raw:
            continue
        m = SCRIPT_LOC_HDR.search(raw)
        if m is None:
            src.append((obj, raw))
        elif m.group(1).decode("utf-8", "replace") == locale:
            loc.append((obj, raw))
    return (loc, True) if loc else (src, False)


def splice_string(raw, pos, old_len, new_text):
    """Replace the length-prefixed string at `pos` (whose payload is `old_len`
    bytes) with `new_text`, fixing the length prefix and 4-byte alignment. The blob
    grows/shrinks freely: Unity-serialized data is inline (no internal byte
    pointers) and `env.file.save()` rebuilds the file-level object-size table."""
    nb = new_text.encode("utf-8")
    ln = len(nb)
    pad_old = (-old_len) % 4
    pad_new = (-ln) % 4
    return (raw[:pos] + struct.pack("<i", ln) + nb + b"\x00" * pad_new
            + raw[pos + 4 + old_len + pad_old:])


# --- shared -----------------------------------------------------------------


def assets_files(data_dir):
    out = []
    for pat in ("resources.assets", "sharedassets*.assets", "globalgamemanagers.assets"):
        out += glob.glob(os.path.join(data_dir, pat))
    # level* scenes can hold TextAssets / scripts too on some builds; include them.
    out += glob.glob(os.path.join(data_dir, "level*"))
    return sorted(set(p for p in out if os.path.isfile(p)))


def read_script(obj):
    d = obj.read()
    name = getattr(d, "m_Name", "") or ""
    s = getattr(d, "m_Script", None)
    if isinstance(s, bytes):
        s = s.decode("utf-8", "surrogateescape")
    return d, name, (s or "")


def doc_locale(script):
    """('en', 'DefaultUI') for a localization doc; ('', name-or-'') for a source doc."""
    m = HEADER_RE.search(script)
    if m:
        return m.group(1), m.group(2)
    return "", ""


def is_managed_text(script):
    if HEADER_RE.search(script):
        return True
    lines = [l for l in script.splitlines() if l.strip()]
    if len(lines) < 2:
        return False
    kv = sum(1 for l in lines if KEY_RE.match(l))
    return kv >= max(2, int(len(lines) * 0.6))


def records(script):
    """Yield (key, value) for each record line, skipping ; headers and blanks."""
    for line in script.splitlines():
        if line.startswith(";") or not line.strip():
            continue
        m = KEY_RE.match(line)
        if m:
            yield m.group(1), m.group(2)


def select_docs(data_dir, locale):
    """[(file, pathId, name, script)] for the managed-text docs of the target locale.

    Prefers localization docs whose header target == locale; if none exist across
    the game, falls back to the source docs (no header)."""
    loc_docs, src_docs = [], []
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        try:
            env = UnityPy.load(path)
        except Exception:
            continue
        for obj in env.objects:
            if obj.type.name != "TextAsset":
                continue
            try:
                _d, name, script = read_script(obj)
            except Exception:
                continue
            if not is_managed_text(script):
                continue
            dst, doc_name = doc_locale(script)
            if name in SKIP_DOCS or doc_name in SKIP_DOCS:
                continue
            entry = (rel, obj.path_id, name, script)
            if dst:
                if dst == locale:
                    loc_docs.append(entry)
            else:
                src_docs.append(entry)
    chosen = loc_docs if loc_docs else src_docs
    chosen.sort(key=lambda e: (e[0], e[1]))
    return chosen


def cmd_export(data_dir, out, locale):
    recs = []
    # tier 1: managed text
    for rel, path_id, name, script in select_docs(data_dir, locale):
        for key, val in records(script):
            recs.append({"t": "mt", "file": rel, "pathId": path_id,
                         "name": name, "key": key, "source": val})
    n_mt = len(recs)
    # tier 2: compiled dialogue
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        try:
            env = UnityPy.load(path)
        except Exception:
            continue
        chosen, localized = script_mbs(env, locale)
        for obj, raw in chosen:
            for idx, (p, L, char, text) in enumerate(dialogue_units(raw, localized)):
                recs.append({"t": "dlg", "file": rel, "pathId": obj.path_id,
                             "idx": idx, "char": char, "source": text})
    with open(out, "w", encoding="utf-8") as f:
        json.dump(recs, f, ensure_ascii=False, indent=1)
    print(f"export: {n_mt} managed-text + {len(recs) - n_mt} dialogue records, locale={locale!r}")


def cmd_import(data_dir, patch_json, out_dir, locale):
    with open(patch_json, encoding="utf-8") as f:
        patch = json.load(f)
    mt = {}                                  # (file, pathId, key) -> translation
    dlg = {}                                 # (file, pathId) -> {idx: translation}
    for r in patch:
        t = r.get("translation")
        if t is None:
            continue
        if r.get("t") == "dlg":
            dlg.setdefault((r["file"], int(r["pathId"])), {})[int(r["idx"])] = t
        else:
            mt[(r["file"], int(r["pathId"]), r["key"])] = t
    changed_files = {k[0] for k in mt} | {k[0] for k in dlg}
    os.makedirs(out_dir, exist_ok=True)

    written = 0
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        if rel not in changed_files:
            continue  # unchanged -> caller keeps the original; do not re-serialize
        env = UnityPy.load(path)
        # Dialogue MBs in this file: the chosen (localization / source) set plus how
        # each is enumerated, so import re-derives the exact idx list export used.
        dlg_localized = {}
        if any(f == rel for (f, _pid) in dlg):
            chosen, localized = script_mbs(env, locale)
            for cobj, _craw in chosen:
                dlg_localized[cobj.path_id] = localized
        n = 0
        for obj in env.objects:
            tn = obj.type.name
            if tn == "TextAsset":
                d, name, script = read_script(obj)
                out_lines, touched = [], False
                for line in script.splitlines():
                    m = None if line.startswith(";") else KEY_RE.match(line)
                    if m and (rel, obj.path_id, m.group(1)) in mt:
                        out_lines.append(f"{m.group(1)}: {mt[(rel, obj.path_id, m.group(1))]}")
                        touched = True
                        n += 1
                    else:
                        out_lines.append(line)
                if touched:
                    d.m_Script = "\n".join(out_lines)
                    d.save()
            elif tn == "MonoBehaviour":
                edits = dlg.get((rel, obj.path_id))
                if not edits or obj.path_id not in dlg_localized:
                    continue
                try:
                    raw = obj.get_raw_data()
                except Exception:
                    continue
                units = dialogue_units(raw, dlg_localized[obj.path_id])
                # splice back-to-front so earlier positions stay valid as lengths change
                for idx in sorted(edits, reverse=True):
                    if idx >= len(units):
                        continue
                    pos, blen, char, _text = units[idx]
                    full = f"{char}: {edits[idx]}" if char else edits[idx]
                    raw = splice_string(raw, pos, blen, full)
                    n += 1
                obj.set_raw_data(raw)
        with open(os.path.join(out_dir, rel), "wb") as f:
            f.write(env.file.save())
        written += 1
        print(f"import: patched {rel} ({n} records)")
    print(f"import: wrote {written} file(s), locale={locale!r}")


# --- TextTable MonoBehaviour engine (unity-textbl) --------------------------
#
# Some Unity (Mono-backend) games keep ALL their text in a couple of custom
# `TextTable` MonoBehaviours inside an Addressables bundle — a per-language string
# matrix. Because the backend is Mono, UnityPy reads AND writes the typetree, so
# (unlike the Naninovel dialogue tier) we edit the structured tree, not raw bytes.
#
#   TextTable typetree:
#     m_languageKeys   : ['Default','ja','zh','zh-tw','ko']   # 0 = base column
#     m_fieldValues[i] : { m_fieldName, m_keys:[0..], m_values:[<per language>] }
#
# We translate `m_values[0]` (the Default column) → the target language, so the
# game shows it whenever its base/Default locale (usually `en`) is active. The
# fingerprint of a TextTable is simply a typetree carrying both `m_languageKeys`
# and `m_fieldValues` (no game DLL / script-class read needed).

def _bundles(aa_dir):
    """The Addressables bundle files under an `aa/` dir (StandaloneWindows64/*.bundle
    for a Windows build, plus any bundle directly in the dir)."""
    out = glob.glob(os.path.join(aa_dir, "StandaloneWindows64", "*.bundle"))
    out += glob.glob(os.path.join(aa_dir, "*.bundle"))
    return sorted(set(p for p in out if os.path.isfile(p)))


def _texttables(env):
    """[(obj, tree)] for every TextTable MonoBehaviour in a loaded bundle."""
    tables = []
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            tree = obj.read_typetree()
        except Exception:
            continue
        if isinstance(tree, dict) and "m_languageKeys" in tree and "m_fieldValues" in tree:
            tables.append((obj, tree))
    return tables


def cmd_texttable_export(aa_dir, out):
    recs = []
    for path in _bundles(aa_dir):
        rel = os.path.basename(path)
        try:
            env = UnityPy.load(path)
        except Exception:
            continue
        for obj, tree in _texttables(env):
            for idx, fld in enumerate(tree.get("m_fieldValues", [])):
                vals = fld.get("m_values") or []
                src = vals[0] if vals else ""
                if not src or not src.strip():
                    continue
                recs.append({"t": "tbl", "file": rel, "pathId": obj.path_id,
                             "idx": idx, "name": fld.get("m_fieldName", ""),
                             "source": src})
    with open(out, "w", encoding="utf-8") as f:
        json.dump(recs, f, ensure_ascii=False, indent=1)
    print(f"texttable-export: {len(recs)} field(s) from {aa_dir}")


def cmd_texttable_import(aa_dir, patch_json, out_dir):
    with open(patch_json, encoding="utf-8") as f:
        patch = json.load(f)
    edits = {}                                # (file, pathId) -> {idx: translation}
    for r in patch:
        t = r.get("translation")
        if t is None:
            continue
        edits.setdefault((r["file"], int(r["pathId"])), {})[int(r["idx"])] = t
    changed_files = {k[0] for k in edits}
    os.makedirs(out_dir, exist_ok=True)

    written = 0
    for path in _bundles(aa_dir):
        rel = os.path.basename(path)
        if rel not in changed_files:
            continue
        env = UnityPy.load(path)
        n = 0
        for obj, tree in _texttables(env):
            per = edits.get((rel, obj.path_id))
            if not per:
                continue
            fvs = tree.get("m_fieldValues", [])
            for idx, tr in per.items():
                if 0 <= idx < len(fvs):
                    vals = fvs[idx].get("m_values")
                    if vals:
                        vals[0] = tr
                        n += 1
            obj.save_typetree(tree)
        blob = None
        for packer in ("lz4", "none"):
            try:
                blob = env.file.save(packer=packer)
                break
            except Exception as e:
                print(f"texttable-import: packer {packer} failed: {e}", file=sys.stderr)
        if blob is None:
            sys.exit(f"texttable-import: could not repack {rel}")
        with open(os.path.join(out_dir, rel), "wb") as f:
            f.write(blob)
        written += 1
        print(f"texttable-import: patched {rel} ({n} field(s))")
    print(f"texttable-import: wrote {written} bundle(s)")


# --- PixelCrushers Dialogue System database (unity-textbl tier 2) -----------
#
# Some TextTable games (e.g. NTR Soccer) also drive their **story dialogue** through
# a **PixelCrushers Dialogue System** `DialogueDatabase` — a single large
# MonoBehaviour in a plain `.assets` file whose typetree is *stripped* (UnityPy can't
# read it structurally). But it serializes as Unity length-prefixed UTF-8 strings, and
# each `DialogueEntry` stores its fields as `[title][value][CustomFieldType_…]`
# triples, so the translatable line is the string that immediately follows a
# `"Dialogue Text"` / `"Menu Text"` **base** title (localized variants carry a locale
# suffix — `"Dialogue Text ja"` — and are left alone; the base holds the shown/English
# text, which we overwrite → Thai). We splice on the raw bytes, exactly like the
# Naninovel dialogue tier (`enum_strings` + `splice_string`), addressing each line by
# its index in a deterministic enumeration so export and import agree.

# DS field titles (the base ones we translate + the non-translatable siblings we must
# NOT mistake a value for).
DS_TITLES = {
    "Title", "Actor", "Conversant", "Menu Text", "Dialogue Text", "Parenthetical",
    "Sequence", "Response Menu Sequence", "Audio Files", "Description", "Articy Id",
    "LinkPriority", "Video File", "Alternate 1", "Group",
}
DS_TRANSLATE_TITLES = ("Dialogue Text", "Menu Text")


def _ds_databases(env):
    """[(obj, raw)] for every DialogueDatabase-like MonoBehaviour in a file: a stripped
    MB whose blob carries the DS field markers."""
    out = []
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            raw = obj.get_raw_data()
        except Exception:
            continue
        if b"Dialogue Text" in raw and b"CustomFieldType" in raw:
            out.append((obj, raw))
    return out


def _ds_units(raw):
    """Ordered [(pos, byte_len, title, text)] of translatable base dialogue in a DS blob.
    The list order is the stable per-MB `idx`. A field serializes as
    `[title][value][CustomFieldType_…]`; the value is the string right after a base
    translate-title, unless it is itself a title/type marker (an empty value, skipped by
    the length-prefixed enumeration, would leave the next title there instead)."""
    strs = enum_strings(raw)
    units = []
    for i, (pos, L, t) in enumerate(strs):
        if t not in DS_TRANSLATE_TITLES or i + 1 >= len(strs):
            continue
        npos, nL, nt = strs[i + 1]
        if nt in DS_TITLES or nt.startswith("CustomFieldType"):
            continue  # empty value — the next string is the following field's title/type
        units.append((npos, nL, t, nt))
    return units


def cmd_dsdb_export(data_dir, out):
    recs = []
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        try:
            env = UnityPy.load(path)
        except Exception:
            continue
        for obj, raw in _ds_databases(env):
            for idx, (_p, _L, title, text) in enumerate(_ds_units(raw)):
                recs.append({"t": "ds", "file": rel, "pathId": obj.path_id,
                             "idx": idx, "title": title, "source": text})
    with open(out, "w", encoding="utf-8") as f:
        json.dump(recs, f, ensure_ascii=False, indent=1)
    print(f"dsdb-export: {len(recs)} dialogue line(s) from {data_dir}")


def cmd_dsdb_import(data_dir, patch_json, out_dir):
    with open(patch_json, encoding="utf-8") as f:
        patch = json.load(f)
    edits = {}                                # (file, pathId) -> {idx: translation}
    for r in patch:
        t = r.get("translation")
        if t is None:
            continue
        edits.setdefault((r["file"], int(r["pathId"])), {})[int(r["idx"])] = t
    changed_files = {k[0] for k in edits}
    os.makedirs(out_dir, exist_ok=True)

    written = 0
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        if rel not in changed_files:
            continue
        env = UnityPy.load(path)
        n = 0
        for obj in env.objects:
            if obj.type.name != "MonoBehaviour":
                continue
            per = edits.get((rel, obj.path_id))
            if not per:
                continue
            try:
                raw = obj.get_raw_data()
            except Exception:
                continue
            units = _ds_units(raw)
            for idx in sorted(per, reverse=True):   # back-to-front: earlier spans stay valid
                if 0 <= idx < len(units):
                    pos, blen, _title, _text = units[idx]
                    raw = splice_string(raw, pos, blen, per[idx])
                    n += 1
            obj.set_raw_data(raw)
        with open(os.path.join(out_dir, rel), "wb") as f:
            f.write(env.file.save())
        written += 1
        print(f"dsdb-import: patched {rel} ({n} line(s))")
    print(f"dsdb-import: wrote {written} file(s)")


def cmd_catalog_crc(catalog_path, out_path=None):
    """Zero every bundle's CRC in an Addressables **JSON** catalog.

    A modified `.bundle` no longer matches the CRC the catalog records for it, and
    Addressables then rejects the load (the game hangs at startup) — the same gate the
    `unity-csvloc` engine defeats in a binary `catalog.bin`. In a JSON catalog the
    per-bundle `AssetBundleRequestOptions` live in `m_ExtraDataString` (base64) as
    **UTF-16LE JSON** strings, each preceded by a 4-byte little-endian byte length:
    `…<i32 len>{"m_Hash":"…","m_Crc":<n>,…}…`. Setting `m_Crc` to 0 disables the check
    (Addressables treats 0 as "don't verify"). We rewrite each such JSON, fixing its
    length prefix, then re-base64 the blob. Non-CRC options are preserved verbatim."""
    import base64 as _b64
    with open(catalog_path, encoding="utf-8") as f:
        cat = json.load(f)
    blob = bytearray(_b64.b64decode(cat.get("m_ExtraDataString", "")))
    needle = '{"m_Hash"'.encode("utf-16-le")
    zeroed = 0
    i = 0
    while True:
        j = blob.find(needle, i)
        if j < 0:
            break
        length = struct.unpack_from("<i", blob, j - 4)[0]   # UTF-16 byte length
        try:
            obj = json.loads(blob[j:j + length].decode("utf-16-le"))
        except Exception:
            i = j + len(needle)
            continue
        if obj.get("m_Crc", 0) == 0:
            i = j + length
            continue
        obj["m_Crc"] = 0
        new = json.dumps(obj, separators=(",", ":"), ensure_ascii=False).encode("utf-16-le")
        blob[j:j + length] = new
        struct.pack_into("<i", blob, j - 4, len(new))
        zeroed += 1
        i = j + len(new)
    cat["m_ExtraDataString"] = _b64.b64encode(bytes(blob)).decode("ascii")
    dst = out_path or catalog_path
    with open(dst, "w", encoding="utf-8") as f:
        json.dump(cat, f, ensure_ascii=False, separators=(",", ":"))
    print(f"catalog-crc: zeroed {zeroed} bundle CRC(s) -> {os.path.basename(dst)}")


def cmd_swap_font(bundle_in, ttf_path, bundle_out):
    """Replace the source TTF of every Dynamic-atlas TMP_FontAsset in an Addressables
    font bundle, so a fallback font renders a script the baked atlases lack.

    A Dynamic TMP font can ship a **pre-baked** glyph/character table + atlas texture
    (e.g. Latin already rasterized). If we only swap the source TTF, those cached
    entries still point at the OLD font's glyphs and the game renders tofu (even the
    previously-fine Latin). So we also **clear each font's baked atlas** — empty the
    glyph/character/used-rect tables and reset the free-rect to the whole atlas — which
    is exactly what TMP's `ClearFontAssetData` does: the runtime then re-rasterizes
    every glyph on demand from the new source. Fonts that already ship an empty atlas
    (the common pure-dynamic case, e.g. Milf Plaza) are unaffected."""
    import UnityPy

    with open(ttf_path, "rb") as f:
        font = f.read()
    env = UnityPy.load(bundle_in)

    # 1) find the dynamic-mode TMP font assets: collect their source Font path_ids AND
    #    clear their baked atlas so the runtime rebuilds it from the swapped source.
    src_ids = set()
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            tree = obj.read_typetree()
        except Exception:
            continue
        if not isinstance(tree, dict) or tree.get("m_AtlasPopulationMode") != 1:
            continue
        src = tree.get("m_SourceFontFile") or {}
        pid = src.get("m_PathID") if isinstance(src, dict) else None
        fid = src.get("m_FileID") if isinstance(src, dict) else None
        if pid and fid in (0, None):
            src_ids.add(pid)
        if tree.get("m_GlyphTable") or tree.get("m_CharacterTable"):
            pad = tree.get("m_AtlasPadding", 0) or 0
            w = tree.get("m_AtlasWidth", 0) or 0
            h = tree.get("m_AtlasHeight", 0) or 0
            tree["m_GlyphTable"] = []
            tree["m_CharacterTable"] = []
            tree["m_UsedGlyphRects"] = []
            tree["m_FreeGlyphRects"] = [
                {"m_X": 0, "m_Y": 0, "m_Width": max(0, w - pad * 2), "m_Height": max(0, h - pad * 2)}
            ]
            tree["m_AtlasTextureIndex"] = 0
            try:
                obj.save_typetree(tree)
            except Exception as e:
                print(f"swap-font: could not clear atlas for {tree.get('m_Name','')}: {e}",
                      file=sys.stderr)

    # 2) swap those Font objects' embedded TTF bytes.
    swapped = 0
    for obj in env.objects:
        if obj.type.name == "Font" and obj.path_id in src_ids:
            d = obj.read()
            d.m_FontData = font
            d.save()
            swapped += 1

    # No dynamic-atlas font here — nothing to do. Don't write an output; the caller
    # tests for the output file's existence and keeps the original bundle. (This lets
    # a caller sweep swap-font over *every* bundle without knowing which hold fonts,
    # e.g. the unity-textbl engine.) Exit 0 so the sweep continues.
    if swapped == 0:
        print(f"swap-font: no dynamic-atlas TMP font in {os.path.basename(bundle_in)} (skipped)")
        return

    # 3) write the bundle. Prefer LZ4 (keeps it ~compressed like the original); fall
    #    back to uncompressed if this UnityPy build can't repack LZ4.
    blob = None
    for packer in ("lz4", "none"):
        try:
            blob = env.file.save(packer=packer)
            break
        except Exception as e:
            print(f"swap-font: packer {packer} failed: {e}", file=sys.stderr)
    if blob is None:
        sys.exit("swap-font: could not repack the bundle")
    with open(bundle_out, "wb") as f:
        f.write(blob)
    print(f"swap-font: swapped {swapped} dynamic font source(s) in {os.path.basename(bundle_in)}")


def main(argv):
    if len(argv) < 2:
        sys.exit("usage: rpgtl_unity.py export|import|swap-font ...")
    cmd = argv[1]
    if cmd == "export":
        locale = argv[4] if len(argv) > 4 else "en"
        cmd_export(argv[2], argv[3], locale)
    elif cmd == "import":
        locale = argv[5] if len(argv) > 5 else "en"
        cmd_import(argv[2], argv[3], argv[4], locale)
    elif cmd == "swap-font":
        cmd_swap_font(argv[2], argv[3], argv[4])
    elif cmd == "texttable-export":
        cmd_texttable_export(argv[2], argv[3])
    elif cmd == "texttable-import":
        cmd_texttable_import(argv[2], argv[3], argv[4])
    elif cmd == "dsdb-export":
        cmd_dsdb_export(argv[2], argv[3])
    elif cmd == "dsdb-import":
        cmd_dsdb_import(argv[2], argv[3], argv[4])
    elif cmd == "catalog-crc":
        cmd_catalog_crc(argv[2], argv[3] if len(argv) > 3 else None)
    else:
        sys.exit(f"unknown command {cmd!r}")


if __name__ == "__main__":
    main(sys.argv)
