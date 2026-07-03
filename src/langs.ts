// Languages offered in the source/target pickers. "Auto" is source-only
// (let the model detect Japanese vs English, common for RPGMaker games).

export const SOURCE_LANGS = [
  "Auto",
  "Japanese",
  "English",
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

// Default to auto-detect: source games vary (Japanese, English, …) and the
// model detects it, so the user rarely needs to change this.
export const DEFAULT_SOURCE = "Auto";
export const DEFAULT_TARGET = "Thai";
