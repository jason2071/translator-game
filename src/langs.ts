// Languages offered in the source/target pickers. "Auto" is source-only and
// chooses an available source in this order: English, Japanese, then Chinese.

export const SOURCE_LANGS = [
  "Auto",
  "English",
  "Japanese",
  "Chinese",
  "Korean",
] as const;

export const TARGET_LANGS = [
  "Thai",
  "English",
  "Japanese",
  "Chinese",
  "Korean",
] as const;

// New imports automatically prefer English, then Japanese, then Chinese when
// an engine exposes multiple source locales.
export const DEFAULT_SOURCE = "Auto";
export const DEFAULT_TARGET = "Thai";
