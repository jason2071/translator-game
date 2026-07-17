// Typed wrappers over the Rust command surface. Keep field names in sync with
// the serde structs in src-tauri (camelCase where those structs rename).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status =
  | "Untranslated"
  | "Failed"
  | "Draft"
  | "Translated"
  | "Reviewed"
  | "Locked";

export const STATUSES: Status[] = [
  "Untranslated",
  "Failed",
  "Draft",
  "Translated",
  "Reviewed",
  "Locked",
];

export interface TransUnit {
  id: number;
  file: string;
  pointer: string;
  kind: string;
  context: string | null;
  group: string | null;
  source: string;
  translation: string | null;
  status: Status;
}

export interface DetectResult {
  engineId: string;
  engineName: string;
  dataDir: string;
  fileCount: number;
  /** Non-blocking advisories to show before import (e.g. a built-in in-game
   * language system that keeps dialogue out of the data files). */
  warnings?: string[];
}

export interface Stats {
  total: number;
  untranslated: number;
  failed: number;
  draft: number;
  translated: number;
  reviewed: number;
  locked: number;
}

export interface ProjectInfo {
  root: string;
  engineId: string;
  engineName: string;
  dataDir: string;
  sourceLang: string;
  targetLang: string;
  gameContext: string;
  era: string;
  stats: Stats;
  freshlyExtracted: boolean;
}

export interface FileCount {
  file: string;
  count: number;
}

export interface ExportResult {
  filesWritten: number;
  unitsApplied: number;
  backupDir: string | null;
  /** How the export was done (e.g. the Ren'Py tl/<lang>/ path); null for in-place. */
  note?: string | null;
}

export interface ModResult {
  /** Absolute path to the written .zip. */
  zipPath: string;
  filesWritten: number;
  unitsApplied: number;
  note?: string | null;
}

export interface RestoreResult {
  filesRestored: number;
  note: string;
}

export interface UnitFilter {
  file?: string;
  status?: Status;
  search?: string;
  /** Columns the `search` substring runs against. Omitted/empty ⇒ source + translation. */
  searchFields?: ("source" | "translation" | "context")[];
  /** Exact speaker/character filter — the sidebar cast dropdown. */
  context?: string;
  untranslatedOnly?: boolean;
  limit?: number;
  offset?: number;
}

export interface GlossaryEntry {
  id: number;
  term: string;
  translation: string;
  note: string | null;
  caseSensitive: boolean;
}

// A speaker → gender row (for gendered Thai particles). `gender` is
// "male" | "female" | "neutral", or "" when a speaker isn't classified yet.
// `note` is a free-text persona/register hint (who they are, how they speak,
// relationships), fed to the Run prompt so pronouns/politeness fit the character.
export interface Character {
  name: string;
  gender: string;
  note: string;
}

// Result of re-scanning the game into an existing project.
export interface RescanResult {
  added: number;
  contextFilled: number;
  total: number;
}

export interface LintWarning {
  unitId: number;
  file: string;
  term: string;
  expected: string;
}

export type ProviderKind =
  | "openai"
  | "openrouter"
  | "local"
  | "anthropic"
  | "gemini";

export interface ProviderConfig {
  kind: ProviderKind;
  baseUrl?: string;
  model: string;
  temperature?: number;
  maxTokens?: number;
  batchSize?: number;
  rpm?: number;
  tone?: string;
  systemPrompt?: string;
  thinking?: boolean;
}

export interface TranslateScope {
  ids?: number[];
  filter?: UnitFilter;
  overwrite?: boolean;
}

export interface TranslateSummary {
  requested: number;
  translated: number;
  reused: number;
  failed: number;
  cancelled: boolean;
  /** First transport-level provider error (unreachable / 401 / 429), if any. */
  error?: string | null;
}

/** A unit filled during a Run, pushed live so the grid updates row-by-row. */
export interface UnitUpdate {
  id: number;
  translation: string | null;
  status: Status;
}

/** A unit that failed during a Run, with the reason, for the errors modal. */
export interface FailedUnit {
  id: number;
  reason: string;
}

export interface GlossCandidate {
  term: string;
  translation: string | null;
  kind: string;
  count: number;
}

export interface Progress {
  done: number;
  total: number;
  translated: number;
  failed: number;
}

export interface TextItem {
  index: number;
  text: string | null;
}

// Live progress of an AI glossary suggest: "mining" (scanning the game locally)
// then "asking" (waiting on the model — `count` = candidates being judged).
export interface GlossSuggestStage {
  stage: "mining" | "asking";
  count: number;
}

export const api = {
  ping: (name: string) => invoke<string>("ping", { name }),

  detectGame: (path: string) =>
    invoke<DetectResult | null>("detect_game", { path }),

  openProject: (path: string, sourceLang?: string, targetLang?: string) =>
    invoke<ProjectInfo>("open_project", { path, sourceLang, targetLang }),

  closeProject: () => invoke<void>("close_project"),

  setLanguages: (source: string, target: string) =>
    invoke<void>("set_languages", { source, target }),
  setGameContext: (text: string) => invoke<void>("set_game_context", { text }),
  setEra: (era: string) => invoke<void>("set_era", { era }),

  listUnits: (filter: UnitFilter) =>
    invoke<TransUnit[]>("list_units", { filter }),
  countUnits: (filter: UnitFilter) =>
    invoke<number>("count_units", { filter }),
  // Bulk-fill filter-matching Untranslated/Failed units with their source text
  // (status → Draft); returns how many rows changed.
  copySourceToTranslation: (filter: UnitFilter) =>
    invoke<number>("copy_source_to_translation", { filter }),

  updateUnit: (id: number, translation: string | null, status: Status) =>
    invoke<void>("update_unit", { id, translation, status }),

  getStats: () => invoke<Stats>("get_stats"),

  listFiles: () => invoke<FileCount[]>("list_files"),

  exportProject: (backup = true, embedFont = false) =>
    invoke<ExportResult>("export_project", { backup, embedFont }),

  /** Export a distributable mod .zip (overlays onto the game; game untouched). */
  exportMod: (embedFont = false) => invoke<ModResult>("export_mod", { embedFont }),

  /** Undo an in-place export: restore the game's original files from .rpgtl/source/. */
  restoreProject: () => invoke<RestoreResult>("restore_original"),

  applyTm: () => invoke<number>("apply_tm"),

  glossaryList: () => invoke<GlossaryEntry[]>("glossary_list"),
  glossaryAdd: (
    term: string,
    translation: string,
    note?: string,
    caseSensitive = false
  ) =>
    invoke<number>("glossary_add", { term, translation, note, caseSensitive }),
  glossaryUpdate: (
    id: number,
    term: string,
    translation: string,
    note?: string,
    caseSensitive = false
  ) =>
    invoke<void>("glossary_update", {
      id,
      term,
      translation,
      note,
      caseSensitive,
    }),
  glossaryDelete: (id: number) => invoke<void>("glossary_delete", { id }),
  glossaryLint: () => invoke<LintWarning[]>("glossary_lint"),
  suggestGlossary: () => invoke<GlossCandidate[]>("suggest_glossary"),
  suggestGlossaryAi: (config: ProviderConfig) =>
    invoke<GlossCandidate[]>("suggest_glossary_ai", { config }),
  suggestGameContext: (config: ProviderConfig) =>
    invoke<string>("suggest_game_context", { config }),
  glossaryAddBulk: (items: [string, string][]) =>
    invoke<number>("glossary_add_bulk", { items }),

  rescanProject: () => invoke<RescanResult>("rescan_project"),

  charactersList: () => invoke<Character[]>("characters_list"),
  characterSet: (name: string, gender: string) =>
    invoke<void>("character_set", { name, gender }),
  characterSetNote: (name: string, note: string) =>
    invoke<void>("character_set_note", { name, note }),
  charactersClear: () => invoke<void>("characters_clear"),
  classifyGenders: (config: ProviderConfig) =>
    invoke<Character[]>("classify_genders", { config }),
  classifyPersonas: (config: ProviderConfig) =>
    invoke<Character[]>("classify_personas", { config }),

  translateUnits: (scope: TranslateScope, config: ProviderConfig) =>
    invoke<TranslateSummary>("translate_units", { scope, config }),
  translateTexts: (texts: string[], config: ProviderConfig) =>
    invoke<(string | null)[]>("translate_texts", { texts, config }),
  rememberTexts: (items: [string, string][]) =>
    invoke<number>("remember_texts", { items }),
  cancelTranslation: () => invoke<void>("cancel_translation"),
  testProvider: (config: ProviderConfig) =>
    invoke<string>("test_provider", { config }),
  listModels: (config: ProviderConfig) =>
    invoke<string[]>("list_models", { config }),

  setKey: (provider: ProviderKind, key: string) =>
    invoke<void>("set_key", { provider, key }),
  hasKey: (provider: ProviderKind) => invoke<boolean>("has_key", { provider }),
  deleteKey: (provider: ProviderKind) =>
    invoke<void>("delete_key", { provider }),

  onProgress: (cb: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>("translate://progress", (e) => cb(e.payload)),

  onTextItem: (cb: (it: TextItem) => void): Promise<UnlistenFn> =>
    listen<TextItem>("translate://item", (e) => cb(e.payload)),

  // Phase updates while an AI glossary suggest runs (scan → ask the model).
  onGlossarySuggest: (cb: (s: GlossSuggestStage) => void): Promise<UnlistenFn> =>
    listen<GlossSuggestStage>("glossary://suggest", (e) => cb(e.payload)),

  // Units filled during a Run, emitted per batch so the grid fills live.
  onUnitsUpdate: (cb: (updates: UnitUpdate[]) => void): Promise<UnlistenFn> =>
    listen<UnitUpdate[]>("translate://units", (e) => cb(e.payload)),

  // Units that failed during a Run (with the reason), emitted per batch.
  onTranslateFailed: (cb: (items: FailedUnit[]) => void): Promise<UnlistenFn> =>
    listen<FailedUnit[]>("translate://failed", (e) => cb(e.payload)),

  // First transport-level error during a Run (AI unreachable / rate-limited).
  onTranslateError: (cb: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("translate://error", (e) => cb(e.payload)),
};
