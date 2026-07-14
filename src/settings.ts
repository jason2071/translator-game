import { create } from "zustand";
import type { ProviderConfig, ProviderKind } from "./ipc";
import { DEFAULT_MAX_LINE_WIDTH } from "./messageWidth";
// The bundled Thai game-translation guidance, shipped as the default Extra prompt
// (single source of truth: prompts/extra.txt, inlined at build via Vite's ?raw).
import DEFAULT_EXTRA_PROMPT from "../prompts/extra.txt?raw";

// Provider configs are non-secret and live in localStorage. API keys never do —
// they go to the OS keychain via the set_key/has_key/delete_key commands.

const KEY = "rpgtl.settings.v1";

export const PROVIDER_LABELS: Record<ProviderKind, string> = {
  openai: "OpenAI",
  openrouter: "OpenRouter",
  local: "Local (Ollama / LM Studio)",
  anthropic: "Claude (Anthropic)",
  gemini: "Gemini (Google)",
};

/** All provider kinds, in display order (top selector + settings tabs). */
export const PROVIDER_KINDS: ProviderKind[] = [
  "local",
  "openai",
  "anthropic",
  "gemini",
  "openrouter",
];

const DEFAULTS: Record<ProviderKind, ProviderConfig> = {
  openai: { kind: "openai", model: "gpt-4o-mini", temperature: 0.3 },
  openrouter: {
    kind: "openrouter",
    model: "openai/gpt-4o-mini",
    temperature: 0.3,
  },
  local: {
    kind: "local",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.1",
    temperature: 0.3,
  },
  anthropic: { kind: "anthropic", model: "claude-sonnet-5", temperature: 0.3 },
  gemini: { kind: "gemini", model: "gemini-2.5-flash", temperature: 0.3 },
};

interface SettingsState {
  active: ProviderKind;
  /** Provider used for glossary auto-translate — independent of the Run provider. */
  glossaryProvider: ProviderKind;
  providers: Record<ProviderKind, ProviderConfig>;
  tone: string;
  systemPrompt: string;
  batchSize: number;
  rpm: number;
  thinking: boolean;
  /** Message-box width limit (half-width chars); 0 disables the overflow guard. */
  maxLineWidth: number;

  setActive: (k: ProviderKind) => void;
  setGlossaryProvider: (k: ProviderKind) => void;
  updateProvider: (k: ProviderKind, patch: Partial<ProviderConfig>) => void;
  setShared: (
    patch: Partial<
      Pick<
        SettingsState,
        "tone" | "systemPrompt" | "batchSize" | "rpm" | "thinking" | "maxLineWidth"
      >
    >
  ) => void;
  /** The full ProviderConfig for a given provider, merged with shared opts. */
  configFor: (kind: ProviderKind) => ProviderConfig;
  /** The full ProviderConfig for the active (Run) provider. */
  activeConfig: () => ProviderConfig;
  /** The full ProviderConfig for the glossary auto-translate provider. */
  glossaryConfig: () => ProviderConfig;
}

// gemini-1.5-* were retired from the API (404). Bump a stale saved model so a
// returning user isn't stuck with a gemini provider that can't reach any model.
function migrateProviders(
  p: Record<ProviderKind, ProviderConfig>
): Record<ProviderKind, ProviderConfig> {
  if (p.gemini?.model?.startsWith("gemini-1.5")) {
    return { ...p, gemini: { ...p.gemini, model: DEFAULTS.gemini.model } };
  }
  return p;
}

function load(): Partial<SettingsState> {
  try {
    return JSON.parse(localStorage.getItem(KEY) || "{}");
  } catch {
    return {};
  }
}

function persist(s: SettingsState) {
  const { active, glossaryProvider, providers, tone, systemPrompt, batchSize, rpm, thinking, maxLineWidth } = s;
  localStorage.setItem(
    KEY,
    JSON.stringify({ active, glossaryProvider, providers, tone, systemPrompt, batchSize, rpm, thinking, maxLineWidth })
  );
}

const saved = load();

export const useSettings = create<SettingsState>((set, get) => ({
  active: saved.active ?? "openai",
  glossaryProvider: saved.glossaryProvider ?? saved.active ?? "openai",
  providers: migrateProviders({ ...DEFAULTS, ...(saved.providers ?? {}) }),
  tone: saved.tone ?? "casual",
  // Seed the bundled default when the Extra prompt is unset/blank (first run, or a
  // returning user who never wrote one — older builds persisted it as ""). A user
  // who wants their own text just overwrites it; clearing it re-shows the default.
  systemPrompt: saved.systemPrompt?.trim() ? saved.systemPrompt : DEFAULT_EXTRA_PROMPT,
  batchSize: saved.batchSize ?? 40,
  rpm: saved.rpm ?? 0,
  thinking: saved.thinking ?? false,
  maxLineWidth: saved.maxLineWidth ?? DEFAULT_MAX_LINE_WIDTH,

  setActive: (k) => {
    set({ active: k });
    persist(get());
  },
  setGlossaryProvider: (k) => {
    set({ glossaryProvider: k });
    persist(get());
  },
  updateProvider: (k, patch) => {
    set({ providers: { ...get().providers, [k]: { ...get().providers[k], ...patch } } });
    persist(get());
  },
  setShared: (patch) => {
    set(patch as Partial<SettingsState>);
    persist(get());
  },
  configFor: (kind) => {
    const s = get();
    return {
      ...s.providers[kind],
      tone: s.tone,
      systemPrompt: s.systemPrompt || undefined,
      batchSize: s.batchSize,
      rpm: s.rpm || undefined,
      thinking: s.thinking,
    };
  },
  activeConfig: () => get().configFor(get().active),
  glossaryConfig: () => get().configFor(get().glossaryProvider),
}));
