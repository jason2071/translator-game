import { create } from "zustand";
import type { ProjectInfo, Stats } from "./ipc";

// Recently-opened projects, so a returning user can reopen a game in one click
// from the import screen. Non-secret metadata only (folder path + engine + stats)
// — like `settings.ts`, it lives in localStorage; API keys never do.

const KEY = "rpgtl.recents.v1";
const CAP = 15;

export interface RecentProject {
  root: string; // absolute folder path — the unique id
  engineId: string;
  engineName: string;
  sourceLang: string;
  targetLang: string;
  stats: Stats; // snapshot from the last successful open
  lastOpened: number; // epoch ms
}

interface RecentsState {
  items: RecentProject[]; // kept sorted newest-first
  add: (info: ProjectInfo) => void;
  remove: (root: string) => void;
  clear: () => void;
}

function load(): RecentProject[] {
  try {
    return JSON.parse(localStorage.getItem(KEY) || "[]");
  } catch {
    return [];
  }
}

function persist(items: RecentProject[]) {
  localStorage.setItem(KEY, JSON.stringify(items));
}

export const useRecents = create<RecentsState>((set, get) => ({
  items: load().sort((a, b) => b.lastOpened - a.lastOpened),

  add: (info) => {
    const entry: RecentProject = {
      root: info.root,
      engineId: info.engineId,
      engineName: info.engineName,
      sourceLang: info.sourceLang,
      targetLang: info.targetLang,
      stats: info.stats,
      lastOpened: Date.now(),
    };
    // Upsert: drop any old entry for this root, put the fresh one first, cap.
    const items = [entry, ...get().items.filter((i) => i.root !== info.root)].slice(0, CAP);
    set({ items });
    persist(items);
  },

  remove: (root) => {
    const items = get().items.filter((i) => i.root !== root);
    set({ items });
    persist(items);
  },

  clear: () => {
    set({ items: [] });
    persist([]);
  },
}));

/** Everything no longer needing work, for a progress read-out. */
export function doneCount(s: Stats): number {
  return s.translated + s.reviewed + s.locked;
}

/** A short "2h ago" style relative time. */
export function timeAgo(ms: number): string {
  const s = Math.max(0, Date.now() - ms) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 604800) return `${Math.floor(s / 86400)}d ago`;
  if (s < 2629800) return `${Math.floor(s / 604800)}w ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** The final path segment (folder name), for display; full path goes in a tooltip. */
export function basename(p: string): string {
  return p.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || p;
}
