#!/usr/bin/env python3
"""Naninovel managed-text bridge for the translator app (Unity engine sidecar).

Reads/writes the translatable strings in a Unity game's `.assets` files. Unity text
lives in built-in `TextAsset` objects, so no game DLL / typetree is needed — UnityPy
alone round-trips them. Naninovel stores UI + localizable strings as "managed text
documents": a TextAsset whose script is `Key: Value` lines, optionally headed by a
`; <src> to <dst> localization document for \`Name\`` comment (a per-locale doc).

Subcommands (driven by `engine/unity.rs`):

  export <data_dir> <manifest.json> [locale]
      Scan the data dir's `.assets`, pick the managed-text docs for the target
      `locale`, emit one JSON record per line: {file, pathId, name, key, source}.

  import <data_dir> <patch.json> <out_dir> [locale]
      Read [{file, pathId, key, translation}], patch the matching records'
      TextAsset.m_Script, and write ONLY the changed `.assets` into <out_dir>
      (mirroring the file name). Untouched files are not emitted — the caller
      leaves the originals in place. Never writes into <data_dir> (the caller may
      pass out_dir == data_dir; UnityPy still holds the source open, so writing a
      distinct dir avoids a Windows sharing violation).

`locale` (default "en") selects the localization docs whose header target is that
locale — the slot the player picks in-game. If a game ships no such localization
docs, it falls back to the source docs (base language becomes the translation).
"""
import sys, os, glob, json, re

# The frozen build (PyInstaller, driven by `scripts/freeze-unity-sidecar.ps1`)
# excludes UnityPy's texture dependencies (PIL, numpy, astc_encoder, …) — this
# engine only touches `TextAsset` and never decodes an image, and those libs are
# ~60 MB of the frozen size. But `import UnityPy` eagerly imports its legacy
# texture patches, which `from PIL import Image` at module load. So under a frozen
# build only, register empty stand-ins for the unused texture libs; the patched
# functions are never called, so the stubs are enough for `import UnityPy` to
# succeed. Under system Python (`sys.frozen` unset) nothing is stubbed and the
# real libraries are used.
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


def assets_files(data_dir):
    out = []
    for pat in ("resources.assets", "sharedassets*.assets", "globalgamemanagers.assets"):
        out += glob.glob(os.path.join(data_dir, pat))
    # level* scenes can hold TextAssets too on some builds; include them.
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
    """[(file, obj, name, script)] for the managed-text docs of the target locale.

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
            # `name` (TextAsset name) is the reliable doc id; the header name is a
            # fallback for source docs whose TextAsset name still equals it.
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
    for rel, path_id, name, script in select_docs(data_dir, locale):
        for key, val in records(script):
            recs.append({"file": rel, "pathId": path_id, "name": name, "key": key, "source": val})
    with open(out, "w", encoding="utf-8") as f:
        json.dump(recs, f, ensure_ascii=False, indent=1)
    print(f"export: {len(recs)} records, locale={locale!r}")


def cmd_import(data_dir, patch_json, out_dir, locale):
    with open(patch_json, encoding="utf-8") as f:
        patch = json.load(f)
    # (file, pathId, key) -> translation
    tr = {}
    for r in patch:
        t = r.get("translation")
        if t is not None:
            tr[(r["file"], int(r["pathId"]), r["key"])] = t
    changed_files = {k[0] for k in tr}
    os.makedirs(out_dir, exist_ok=True)

    written = 0
    for path in assets_files(data_dir):
        rel = os.path.basename(path)
        if rel not in changed_files:
            continue  # unchanged -> caller keeps the original; do not re-serialize
        env = UnityPy.load(path)
        n = 0
        for obj in env.objects:
            if obj.type.name != "TextAsset":
                continue
            d, name, script = read_script(obj)
            out_lines, touched = [], False
            for line in script.splitlines():
                m = None if line.startswith(";") else KEY_RE.match(line)
                if m and (rel, obj.path_id, m.group(1)) in tr:
                    out_lines.append(f"{m.group(1)}: {tr[(rel, obj.path_id, m.group(1))]}")
                    touched = True
                    n += 1
                else:
                    out_lines.append(line)
            if touched:
                d.m_Script = "\n".join(out_lines)
                d.save()
        with open(os.path.join(out_dir, rel), "wb") as f:
            f.write(env.file.save())
        written += 1
        print(f"import: patched {rel} ({n} records)")
    print(f"import: wrote {written} file(s), locale={locale!r}")


def main(argv):
    if len(argv) < 2:
        sys.exit("usage: rpgtl_unity.py export|import ...")
    cmd = argv[1]
    if cmd == "export":
        locale = argv[4] if len(argv) > 4 else "en"
        cmd_export(argv[2], argv[3], locale)
    elif cmd == "import":
        locale = argv[5] if len(argv) > 5 else "en"
        cmd_import(argv[2], argv[3], argv[4], locale)
    else:
        sys.exit(f"unknown command {cmd!r}")


if __name__ == "__main__":
    main(sys.argv)
