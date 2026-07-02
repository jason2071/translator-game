// Typed wrappers over the Rust command surface. Keep field names in sync with
// the serde structs in src-tauri (camelCase where those structs rename).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status =
  | "Untranslated"
  | "Draft"
  | "Translated"
  | "Reviewed"
  | "Locked";

export const STATUSES: Status[] = [
  "Untranslated",
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
}

export interface Stats {
  total: number;
  untranslated: number;
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
}

export interface UnitFilter {
  file?: string;
  status?: Status;
  search?: string;
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
}

export interface TranslateScope {
  ids?: number[];
  filter?: UnitFilter;
  overwrite?: boolean;
}

export interface TranslateSummary {
  requested: number;
  translated: number;
  failed: number;
  cancelled: boolean;
}

export interface Progress {
  done: number;
  total: number;
  translated: number;
  failed: number;
}

export const api = {
  ping: (name: string) => invoke<string>("ping", { name }),

  detectGame: (path: string) =>
    invoke<DetectResult | null>("detect_game", { path }),

  openProject: (path: string, sourceLang?: string, targetLang?: string) =>
    invoke<ProjectInfo>("open_project", { path, sourceLang, targetLang }),

  closeProject: () => invoke<void>("close_project"),

  listUnits: (filter: UnitFilter) =>
    invoke<TransUnit[]>("list_units", { filter }),

  updateUnit: (id: number, translation: string | null, status: Status) =>
    invoke<void>("update_unit", { id, translation, status }),

  getStats: () => invoke<Stats>("get_stats"),

  listFiles: () => invoke<FileCount[]>("list_files"),

  exportProject: (backup = true) =>
    invoke<ExportResult>("export_project", { backup }),

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

  translateUnits: (scope: TranslateScope, config: ProviderConfig) =>
    invoke<TranslateSummary>("translate_units", { scope, config }),
  cancelTranslation: () => invoke<void>("cancel_translation"),
  testProvider: (config: ProviderConfig) =>
    invoke<string>("test_provider", { config }),

  setKey: (provider: ProviderKind, key: string) =>
    invoke<void>("set_key", { provider, key }),
  hasKey: (provider: ProviderKind) => invoke<boolean>("has_key", { provider }),
  deleteKey: (provider: ProviderKind) =>
    invoke<void>("delete_key", { provider }),

  onProgress: (cb: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>("translate://progress", (e) => cb(e.payload)),
};
