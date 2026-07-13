// Estimate how wide a message line renders in an RPGMaker message box and flag
// lines that overflow. RPGMaker draws full-width (CJK) glyphs at ~2x the width
// of half-width (Latin/Thai) ones, and combining marks (Thai vowels/tones)
// take no horizontal space. Control codes (\C[n], \N[1], masked ⟦n⟧ sentinels,
// face/name escapes) draw nothing, so they are stripped before counting.
//
// This is a heuristic: real width also depends on the font, \FS size changes,
// and whether a face graphic narrows the box. It is meant as an advisory guard,
// not an exact renderer.

// Inline codes draw nothing, so they are stripped before counting width. Each
// engine has its own grammar; masked ⟦n⟧ sentinels are stripped for all.
// RPGMaker: \C[2], \V[7], \FS[24], \N[1], \., \!, \\ …
const CODE_RE = /\\[A-Za-z]+(?:\[[^\]]*\])?|\\[^A-Za-z]|%\d+|⟦\d+⟧/g;
// Ren'Py: [interpolation], {text tags}, backslash escapes.
const RENPY_CODE_RE = /\\.|\[[^[\]]+\]|\{[^{}]+\}|⟦\d+⟧/g;
// TyranoScript / KiriKiri KAG: [tags], backslash escapes.
const TYRANO_CODE_RE = /\\.|\[[^\]]*\]|⟦\d+⟧/g;
// Godot: BBCode [tag], format braces, printf conversions, backslash escapes.
const GODOT_CODE_RE = /\\.|\[[^\]]+\]|\{[^{}]+\}|%(?:\d+\$)?[-+0#]*\d*(?:\.\d+)?[sdifgeExXoc]|%%|⟦\d+⟧/g;
// Forger .acod: shape-based angle tags (open vocabulary incl. <LF>), {variable},
// [bracket] (no nesting), printf (no space flag). No backslash. Mirrors mask_forger.
const FORGER_CODE_RE = /<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\{[^{}]+\}|\[[^[\]]+\]|%(?:\d+\$)?[-+0#]*\d*(?:\.\d+)?[sdifgeExXoc]|%%|⟦\d+⟧/g;
// AC Origins aclocexport text: shape-based angle tags + [cue] brackets only
// (no {…}, no %). Mirrors mask_ac_loctext.
const AC_LOCTEXT_CODE_RE = /<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\[[^[\]]+\]|⟦\d+⟧/g;
// Unity/Naninovel: TMPro rich-text tags, {n} format args, backslash escapes (no
// `[…]`/`%`). Mirrors mask_unity.
const UNITY_CODE_RE = /\\.|<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\{[^{}]+\}|⟦\d+⟧/g;

function codeRe(engineId?: string | null): RegExp {
  if (engineId === "renpy") return RENPY_CODE_RE;
  if (engineId === "tyrano" || engineId === "kirikiri") return TYRANO_CODE_RE;
  if (engineId === "godot") return GODOT_CODE_RE;
  if (engineId === "forger-acod") return FORGER_CODE_RE;
  if (engineId === "ac-loctext") return AC_LOCTEXT_CODE_RE;
  if (engineId === "unity" || engineId === "unity-csvloc" || engineId === "unity-textbl") return UNITY_CODE_RE;
  return CODE_RE;
}

/** True for glyphs RPGMaker draws at double width (CJK ideographs, kana, …). */
function isFullWidth(cp: number): boolean {
  return (
    (cp >= 0x1100 && cp <= 0x115f) || // Hangul Jamo
    (cp >= 0x2e80 && cp <= 0x303e) || // CJK radicals / Kangxi / punctuation
    (cp >= 0x3041 && cp <= 0x33ff) || // Hiragana, Katakana, CJK symbols
    (cp >= 0x3400 && cp <= 0x4dbf) || // CJK Ext A
    (cp >= 0x4e00 && cp <= 0x9fff) || // CJK Unified
    (cp >= 0xa000 && cp <= 0xa4cf) || // Yi
    (cp >= 0xac00 && cp <= 0xd7a3) || // Hangul syllables
    (cp >= 0xf900 && cp <= 0xfaff) || // CJK compatibility
    (cp >= 0xfe30 && cp <= 0xfe4f) || // CJK compatibility forms
    (cp >= 0xff00 && cp <= 0xff60) || // Fullwidth forms
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x20000 && cp <= 0x3fffd) // CJK Ext B+
  );
}

/** True for combining marks that add no horizontal width (Thai, diacritics). */
function isZeroWidth(cp: number): boolean {
  return (
    (cp >= 0x0300 && cp <= 0x036f) || // combining diacritical marks
    cp === 0x0e31 ||
    (cp >= 0x0e34 && cp <= 0x0e3a) || // Thai above/below vowels
    (cp >= 0x0e47 && cp <= 0x0e4e) || // Thai tone marks & signs
    (cp >= 0x200b && cp <= 0x200f) // zero-width spaces / marks
  );
}

/** Roughly the on-screen width of one line, in half-width character units. */
function displayWidth(line: string, engineId?: string | null): number {
  let w = 0;
  for (const ch of line.replace(codeRe(engineId), "")) {
    const cp = ch.codePointAt(0)!;
    if (isZeroWidth(cp)) continue;
    w += isFullWidth(cp) ? 2 : 1;
  }
  return w;
}

export interface Overflow {
  /** 1-based line number within the translation. */
  line: number;
  /** Estimated display width of that line. */
  width: number;
}

/** Per-line overflow for a (possibly multi-line) translation, given a limit. */
export function overflowLines(
  text: string,
  max: number,
  engineId?: string | null
): Overflow[] {
  if (!text || max <= 0) return [];
  const out: Overflow[] = [];
  text.split("\n").forEach((ln, i) => {
    const w = displayWidth(ln, engineId);
    if (w > max) out.push({ line: i + 1, width: w });
  });
  return out;
}

/** Default box width guess for the RPGMaker MZ default message window/font. */
export const DEFAULT_MAX_LINE_WIDTH = 46;
