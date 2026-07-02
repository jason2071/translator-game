import { create } from "zustand";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type Progress } from "./ipc";

// One shared translation status for the whole app. Both the main Run
// (translate_units) and the glossary "Translate empty" (translate_texts) go
// through `run()`, so they share ONE progress bar, emit on the same
// `translate://progress` channel, and can never overlap — only one runs at a
// time. Any view can read `busy`/`progress` to show the same status.

export type RunKind = "units" | "glossary";

interface TranslationState {
  busy: boolean;
  kind: RunKind | null;
  progress: Progress | null;

  // Subscribe to progress, mark busy, run `fn`, then clean up. Throws if a
  // translation is already running (callers also disable their buttons on busy).
  run: <T>(kind: RunKind, fn: () => Promise<T>) => Promise<T>;
  cancel: () => void;
}

let unlisten: UnlistenFn | null = null;

export const useTranslation = create<TranslationState>((set, get) => ({
  busy: false,
  kind: null,
  progress: null,

  run: async (kind, fn) => {
    if (get().busy) throw new Error("A translation is already running");
    set({ busy: true, kind, progress: null });
    unlisten = await api.onProgress((p) => set({ progress: p }));
    try {
      return await fn();
    } finally {
      unlisten?.();
      unlisten = null;
      set({ busy: false, kind: null, progress: null });
    }
  },

  cancel: () => void api.cancelTranslation(),
}));
