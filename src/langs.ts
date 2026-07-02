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

export const DEFAULT_SOURCE = "Japanese";
export const DEFAULT_TARGET = "Thai";
