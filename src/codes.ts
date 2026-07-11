// Engine-aware inline-code checks. Each engine has its own inline codes that
// must survive translation unchanged; codesMismatch warns when a translation
// drops, duplicates, or alters the codes present in the source.

// RPGMaker: \C[2], \V[7], \FS[24], \N[1], \., \!, \\ …
const RPGMAKER_RE = /\\[A-Za-z]+(?:\[[^\]]*\])?|\\[^A-Za-z]|%\d+/g;
// Ren'Py: [interpolation], {text tags}, and backslash escapes (\", \n). Escaped
// [[ / {{ are literal text, so a bare doubled bracket contributes no code.
const RENPY_RE = /\\.|\[[^[\]]+\]|\{[^{}]+\}/g;
// TyranoScript / KiriKiri KAG: [tags] (inline and block) and backslash escapes.
const TYRANO_RE = /\\.|\[[^\]]*\]/g;
// Godot: BBCode [tag], String.format braces {0}/{name}, printf %s/%d/%.2f/%1$s,
// and backslash escapes. (No space flag: "50% off" is prose, not `% o`.)
const GODOT_RE = /\\.|\[[^\]]+\]|\{[^{}]+\}|%(?:\d+\$)?[-+0#]*\d*(?:\.\d+)?[sdifgeExXoc]|%%/g;
// Forger .acod: HTML-ish angle tags by shape (open vocabulary incl. <LF>) — a
// name then either `=` attributes or a bare/self-closing token, so prose like
// "5 < 10" or "<low then flee>" isn't a code; {variable}, [bracket] (no nesting,
// mirroring the backend), and printf %s/%d. No backslash. Mirrors mask_forger.
const FORGER_RE = /<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\{[^{}]+\}|\[[^[\]]+\]|%(?:\d+\$)?[-+0#]*\d*(?:\.\d+)?[sdifgeExXoc]|%%/g;
// AC Origins aclocexport text: shape-based angle tags (incl. <LF>/<CR>) and [cue]
// audio brackets only. Unlike Forger, {…} (a whole-line wrap) and % (prose) are
// NOT codes here. Mirrors mask_ac_loctext.
const AC_LOCTEXT_RE = /<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\[[^[\]]+\]/g;
// Unity/Naninovel managed text: TMPro rich-text tags (shape-based), {n}/{name}
// format args, and backslash escapes. `[…]` and `%` are prose here, not codes.
// Mirrors mask_unity.
const UNITY_RE = /\\.|<\s*\/?\s*[A-Za-z][A-Za-z0-9]*(?:[^<>]*=[^<>]*)?\s*\/?>|\{[^{}]+\}/g;

function codeRe(engineId?: string | null): RegExp {
  if (engineId === "renpy") return RENPY_RE;
  if (engineId === "tyrano" || engineId === "kirikiri") return TYRANO_RE;
  if (engineId === "godot") return GODOT_RE;
  if (engineId === "forger-acod") return FORGER_RE;
  if (engineId === "ac-loctext") return AC_LOCTEXT_RE;
  if (engineId === "unity") return UNITY_RE;
  return RPGMAKER_RE;
}

/** The sorted signature of inline codes in a string, for equality comparison. */
function codeKey(s: string, re: RegExp): string {
  return (s.match(re) ?? []).slice().sort().join(" ");
}

/** True if the translation's set of inline codes differs from the source's. */
export function codesMismatch(
  source: string,
  translation: string | null,
  engineId?: string | null
): boolean {
  if (!translation) return false;
  const re = codeRe(engineId);
  return codeKey(source, re) !== codeKey(translation, re);
}
