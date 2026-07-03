// Engine-aware inline-code checks. Each engine has its own inline codes that
// must survive translation unchanged; codesMismatch warns when a translation
// drops, duplicates, or alters the codes present in the source.

// RPGMaker: \C[2], \V[7], \FS[24], \N[1], \., \!, \\ …
const RPGMAKER_RE = /\\[A-Za-z]+(?:\[[^\]]*\])?|\\[^A-Za-z]/g;
// Ren'Py: [interpolation], {text tags}, and backslash escapes (\", \n). Escaped
// [[ / {{ are literal text, so a bare doubled bracket contributes no code.
const RENPY_RE = /\\.|\[[^[\]]+\]|\{[^{}]+\}/g;

function codeRe(engineId?: string | null): RegExp {
  return engineId === "renpy" ? RENPY_RE : RPGMAKER_RE;
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
