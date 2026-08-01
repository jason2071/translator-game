#!/usr/bin/env python3
"""Seed XUnity.AutoTranslator translation files from a Unity game's I2 Localization data.

Why this exists
---------------
Unity is out of scope for this app as a *file format* (see docs/ENGINES.md); the
route for a Unity game is XUnity.AutoTranslator, whose translation files the
`xunity` engine reads and writes. But XUnity only ever learns a line once the game
actually puts it on screen, so a fresh install yields a few dozen entries — you'd
have to walk every scene, quest and menu to collect the rest.

Most Unity games that ship many languages use the **I2 Localization** asset, which
keeps every term and every language in one serialized object. This script reads
that object directly and writes the whole table out as XUnity translation files,
so the app sees the entire game at once instead of whatever happened to render.

Nothing is written into the game's own assets: the output is a plain text file in
XUnity's translation folder, which XUnity reloads with ALT+R.

    game assets ──this script──► BepInEx/Translation/<lang>/Text/i2_dump.txt
       [ app: open folder → translate → export ]
    game ◄──XUnity reload── same file, now filled in

Usage
-----
    python scripts/i2_to_xunity.py <game-folder> [--from English] [--to Thai]
    python scripts/i2_to_xunity.py <game-folder> --list        # show languages only
    python scripts/i2_to_xunity.py <game-folder> --out custom/path.txt

Requires UnityPy (`pip install UnityPy`). Read-only on the game's assets.

Format notes (I2 `LanguageSourceData`, Unity binary serialization)
-----------------------------------------------------------------
    ... header ...
    i32   term count
    repeat:
        string  Term            e.g. "HFail/Fail"
        i32     TermType
        i32     language count
        string  ×count          one per language, in mLanguages order
        i32     flag byte count
        bytes   ×count
    i32   language count
    repeat:
        string  Name            e.g. "Thai"
        string  Code            e.g. "th"
        ... per-language fields ...

The header length varies by I2 version, so the term list is found by trying each
4-byte-aligned offset after `m_Name` and keeping the one that parses the most
terms — cheap, and it survives a version bump that a hard-coded offset would not.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

try:
    import UnityPy
except ImportError:  # pragma: no cover - user-facing dependency check
    sys.exit("UnityPy is required: pip install UnityPy")


class Reader:
    """Unity binary reader: length-prefixed UTF-8 strings, 4-byte aligned."""

    def __init__(self, buf: bytes, pos: int = 0):
        self.b = buf
        self.i = pos

    def i32(self) -> int:
        v = struct.unpack_from("<i", self.b, self.i)[0]
        self.i += 4
        return v

    def align(self) -> None:
        self.i += (-self.i) % 4

    def string(self) -> str:
        n = self.i32()
        if n < 0 or self.i + n > len(self.b):
            raise ValueError(f"bad string length {n}")
        s = self.b[self.i : self.i + n].decode("utf-8")
        self.i += n
        self.align()
        return s


def parse_terms(raw: bytes, start: int, limit: int | None = None):
    """Parse the term list at `start`. Returns (terms, end_offset).

    A term is (key, [translation per language]). Raises on the first malformed
    record, which is what makes `find_terms` able to reject a wrong offset.
    """
    r = Reader(raw, start)
    count = r.i32()
    if count <= 0 or count > 500_000:
        raise ValueError(f"implausible term count {count}")
    if limit is not None:
        count = min(count, limit)
    terms = []
    for _ in range(count):
        key = r.string()
        r.i32()  # TermType
        n_lang = r.i32()
        if n_lang < 0 or n_lang > 200:
            raise ValueError(f"implausible language count {n_lang}")
        langs = [r.string() for _ in range(n_lang)]
        n_flags = r.i32()
        if n_flags < 0 or r.i + n_flags > len(raw):
            raise ValueError(f"implausible flag count {n_flags}")
        r.i += n_flags
        r.align()
        # I2 versions differ in what trails a term (Languages_Touch, per-term
        # metadata). Rather than model every variant, skip the zero words until the
        # next record's key length lines up.
        while r.i + 8 <= len(raw) and struct.unpack_from("<i", raw, r.i)[0] == 0:
            r.i += 4
        terms.append((key, langs))
    return terms, r.i


def find_terms(raw: bytes, name_end: int):
    """Locate and parse the term list, trying each aligned offset after m_Name."""
    best = None
    for off in range(name_end, min(name_end + 256, len(raw) - 8), 4):
        try:
            probe, _ = parse_terms(raw, off, limit=4)
        except Exception:
            continue
        if not probe or not probe[0][1]:
            continue
        try:
            terms, end = parse_terms(raw, off)
        except Exception:
            continue
        if best is None or len(terms) > len(best[0]):
            best = (terms, end)
    if best is None:
        raise ValueError("could not locate the I2 term list")
    return best


def scan_strings(raw: bytes, start: int) -> list[str]:
    """Every length-prefixed UTF-8 string from `start` on, in order."""
    out, i, n = [], start, len(raw)
    while i + 4 <= n:
        ln = struct.unpack_from("<i", raw, i)[0]
        if 0 < ln <= 4096 and i + 4 + ln <= n:
            try:
                s = raw[i + 4 : i + 4 + ln].decode("utf-8")
            except UnicodeDecodeError:
                i += 1
                continue
            if s and all(c in "\n\r\t" or c.isprintable() for c in s):
                out.append(s)
                i += 4 + ln
                i += (-i) % 4
                continue
        i += 1
    return out


def looks_like_code(s: str) -> bool:
    """`en`, `th`, `zh-TW`, `es-US` — an ISO language code, not a language name."""
    if not 2 <= len(s) <= 6 or not s[:2].isalpha() or not s[:2].islower():
        return False
    return len(s) == 2 or (s[2] == "-" and s[3:].isalnum())


def parse_languages(raw: bytes, start: int, n_expected: int) -> list[tuple[str, str]]:
    """Read the (name, code) language list that follows the terms.

    The per-language trailing fields differ between I2 versions, so instead of
    modelling them this walks the remaining strings and pairs each name with the
    code that follows it — a language whose code was dropped still keeps its name
    and its position, which is all the caller needs.
    """
    strs = scan_strings(raw, start)
    out: list[tuple[str, str]] = []
    i = 0
    while i < len(strs) and len(out) < n_expected:
        name = strs[i]
        if looks_like_code(name):  # a stray code with no name before it
            i += 1
            continue
        code = ""
        if i + 1 < len(strs) and looks_like_code(strs[i + 1]):
            code = strs[i + 1]
            i += 1
        out.append((name, code))
        i += 1
    if len(out) < n_expected:
        raise ValueError(f"found {len(out)} languages, expected {n_expected}")
    return out[:n_expected]


def load_i2(root: Path):
    """Find the I2 `I2Languages` object in a game folder and parse it.

    Returns (languages, terms). Scans the Unity serialized files most likely to
    hold it first, then every bundle.
    """
    data_dirs = [p for p in root.iterdir() if p.is_dir() and p.name.endswith("_Data")]
    candidates: list[Path] = []
    for d in data_dirs or [root]:
        for name in ("resources.assets", "sharedassets0.assets", "globalgamemanagers.assets"):
            p = d / name
            if p.is_file():
                candidates.append(p)
        candidates += sorted(d.glob("*.assets"))
        candidates += sorted(d.glob("StreamingAssets/aa/**/*.bundle"))
    seen = set()
    for path in candidates:
        if path in seen:
            continue
        seen.add(path)
        try:
            env = UnityPy.load(str(path))
        except Exception:
            continue
        for obj in env.objects:
            if obj.type.name != "MonoBehaviour":
                continue
            try:
                raw = obj.get_raw_data()
            except Exception:
                continue
            if b"I2Languages" not in raw[:256]:
                continue
            marker = raw.find(b"I2Languages")
            name_end = marker + len("I2Languages")
            name_end += (-name_end) % 4
            try:
                terms, end = find_terms(raw, name_end)
            except Exception as e:
                print(f"  {path.name}: found I2Languages but could not parse it ({e})")
                continue
            n_lang = max((len(v) for _, v in terms), default=0)
            try:
                languages = parse_languages(raw, end, n_lang)
            except Exception:
                languages = [(f"lang{i}", "") for i in range(n_lang)]
            print(f"  source: {path.name} ({len(terms)} terms x {n_lang} languages)")
            return languages, terms
    raise SystemExit(
        "no I2 Localization data found — this game may use a different localization "
        "system. Walk the game with XUnity installed instead; the app reads whatever "
        "it collects."
    )


def pick(languages: list[tuple[str, str]], wanted: str) -> int:
    w = wanted.strip().lower()
    for i, (name, code) in enumerate(languages):
        if name.lower() == w or code.lower() == w:
            return i
    for i, (name, code) in enumerate(languages):
        if w in name.lower():
            return i
    raise SystemExit(
        f"language {wanted!r} not found. Available: "
        + ", ".join(f"{n} ({c})" for n, c in languages)
    )


def escape(s: str) -> str:
    """XUnity entries are one line: a real break becomes the two-character \\n."""
    return s.replace("\r\n", "\\n").replace("\n", "\\n").replace("\r", "\\n")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("game", type=Path, help="the game folder (holds the .exe)")
    ap.add_argument("--from", dest="src", default="English", help="source language (default: English)")
    ap.add_argument("--to", dest="dst", default="Thai", help="target language (default: Thai)")
    ap.add_argument("--out", type=Path, default=None, help="output file (default: BepInEx/Translation/<code>/Text/i2_dump.txt)")
    ap.add_argument("--list", action="store_true", help="only list the languages found")
    args = ap.parse_args()

    if not args.game.is_dir():
        sys.exit(f"not a folder: {args.game}")

    print(f"scanning {args.game}")
    languages, terms = load_i2(args.game)

    if args.list:
        for i, (name, code) in enumerate(languages):
            filled = sum(1 for _, v in terms if i < len(v) and v[i].strip())
            print(f"  [{i:2}] {name:24} {code:6} {filled:5}/{len(terms)} filled")
        return

    si, di = pick(languages, args.src), pick(languages, args.dst)
    code = languages[di][1] or languages[di][0].lower()
    out = args.out or args.game / "BepInEx" / "Translation" / code / "Text" / "i2_dump.txt"

    lines, filled, skipped = [], 0, 0
    seen: set[str] = set()
    for _key, vals in terms:
        src = vals[si].strip() if si < len(vals) else ""
        dst = vals[di].strip() if di < len(vals) else ""
        # XUnity matches on the text the game displays, so the source string is the
        # key. No source text, nothing to match against.
        if not src or src in seen:
            skipped += 1
            continue
        seen.add(src)
        if dst:
            filled += 1
        lines.append(f"{escape(src)}={escape(dst)}")

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")

    print(f"wrote {out}")
    print(f"  {len(lines)} entries — {filled} already translated, {len(lines) - filled} to do")
    if skipped:
        print(f"  ({skipped} terms skipped: empty or duplicate source text)")
    print()
    print("Next: open the game folder in the app (it detects as `xunity`), Run, Export,")
    print("then press ALT+R in game to reload the translation.")


if __name__ == "__main__":
    main()
