import { create } from "zustand";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type Progress } from "./ipc";

// One shared translation engine for the whole app. Jobs (the main Run over
// units, and the glossary "Translate empty") run ONE AT A TIME on the same
// translate://progress channel — the backend cancel flag is global, so they
// must serialize. Instead of rejecting a second request, we QUEUE it: it starts
// automatically when the running job finishes.
//
// Status is tracked PER KIND so the main screen can show the Run and glossary
// states side by side, while the glossary modal shows only its own.

export type RunKind = "units" | "glossary";
export type Phase = "idle" | "queued" | "running";

export interface KindStatus {
  phase: Phase;
  progress: Progress | null;
}

interface Job {
  kind: RunKind;
  fn: () => Promise<unknown>;
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
}

interface TranslationState {
  units: KindStatus;
  glossary: KindStatus;
  active: RunKind | null; // the job currently running (owns the progress channel)

  // Queue a job; resolves with fn's result when it eventually runs. Rejects if
  // that kind is already queued/running (callers also disable their buttons).
  enqueue: <T>(kind: RunKind, fn: () => Promise<T>) => Promise<T>;
  // Cancel the running job (via the backend flag) or drop it from the queue.
  cancel: (kind: RunKind) => void;
}

const IDLE: KindStatus = { phase: "idle", progress: null };

let queue: Job[] = [];
let unlisten: UnlistenFn | null = null;

export const useTranslation = create<TranslationState>((set, get) => {
  const setKind = (kind: RunKind, patch: Partial<KindStatus>) =>
    set((s) =>
      kind === "units"
        ? { units: { ...s.units, ...patch } }
        : { glossary: { ...s.glossary, ...patch } }
    );

  const pump = async () => {
    if (get().active) return; // one at a time
    const job = queue[0];
    if (!job) return;
    set({ active: job.kind });
    setKind(job.kind, { phase: "running", progress: null });
    // Only the running job listens, so progress lands on the right kind.
    unlisten = await api.onProgress((p) => setKind(job.kind, { progress: p }));
    try {
      job.resolve(await job.fn());
    } catch (e) {
      job.reject(e);
    } finally {
      unlisten?.();
      unlisten = null;
      queue.shift();
      setKind(job.kind, { phase: "idle", progress: null });
      set({ active: null });
      pump(); // start the next queued job, if any
    }
  };

  return {
    units: { ...IDLE },
    glossary: { ...IDLE },
    active: null,

    enqueue: <T,>(kind: RunKind, fn: () => Promise<T>) => {
      if (get()[kind].phase !== "idle") {
        return Promise.reject(new Error("already running")) as Promise<T>;
      }
      return new Promise<T>((resolve, reject) => {
        queue.push({ kind, fn, resolve: resolve as (v: unknown) => void, reject });
        setKind(kind, { phase: "queued", progress: null });
        pump();
      });
    },

    cancel: (kind) => {
      if (get().active === kind) {
        api.cancelTranslation(); // backend stops the running job; it resolves partial
        return;
      }
      const idx = queue.findIndex((j) => j.kind === kind);
      if (idx >= 0) {
        const [job] = queue.splice(idx, 1);
        setKind(kind, { phase: "idle", progress: null });
        job.reject(new Error("cancelled"));
      }
    },
  };
});
