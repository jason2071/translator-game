import { create } from "zustand";
import {
  api,
  type FileCount,
  type ProjectInfo,
  type Stats,
  type Status,
  type TransUnit,
  type UnitFilter,
  type UnitUpdate,
} from "./ipc";
import { useGlossarySuggest } from "./glossarySuggest";
import { useRecents } from "./recents";
import { useErrors } from "./errors";

/** A contiguous slice of the filtered unit list, starting at absolute `offset`. */
interface UnitWindow {
  offset: number;
  rows: TransUnit[];
}

interface AppStore {
  project: ProjectInfo | null;
  files: FileCount[];
  stats: Stats | null;
  /** Total units matching the current filter (the virtualizer's row count). */
  total: number;
  /** The loaded window around the current scroll position (see WINDOW/MARGIN). */
  window: UnitWindow;
  filter: UnitFilter;
  loading: boolean;
  error: string | null;

  openProject: (path: string, sourceLang?: string, targetLang?: string) => Promise<void>;
  closeProject: () => Promise<void>;
  setLanguages: (source: string, target: string) => Promise<void>;
  setFilter: (patch: Partial<UnitFilter>) => Promise<void>;
  reloadUnits: () => Promise<void>;
  refreshMeta: () => Promise<void>;
  refreshStats: () => Promise<void>;
  /** Re-count only (post-Run) — updates the scrollbar size without refetching. */
  refreshTotal: () => Promise<void>;
  /** Fetch a fresh window if the visible range is outside/near the loaded one. */
  ensureWindow: (start: number, end: number) => void;
  editUnit: (id: number, translation: string, status?: Status) => Promise<void>;
  setStatus: (id: number, status: Status) => Promise<void>;
  applyUnitUpdates: (updates: UnitUpdate[]) => void;
}

// The grid is windowed: we hold only a slice of the filtered list in memory, so a
// 1M-unit project stays light. WINDOW rows are fetched at a time; a new window is
// fetched once the visible range comes within MARGIN of the loaded slice's edge.
const WINDOW = 400;
const MARGIN = 100;

// Guards window writes: reloadUnits and ensureWindow each bump this before their
// fetch and only apply their result if still the latest — so a stale fetch (old
// filter or superseded scroll) never overwrites a newer window.
let winReq = 0;

const EMPTY_WINDOW: UnitWindow = { offset: 0, rows: [] };

export const useStore = create<AppStore>((set, get) => ({
  project: null,
  files: [],
  stats: null,
  total: 0,
  window: EMPTY_WINDOW,
  filter: {},
  loading: false,
  error: null,

  openProject: async (path, sourceLang, targetLang) => {
    set({ loading: true, error: null });
    try {
      const project = await api.openProject(path, sourceLang, targetLang);
      useRecents.getState().add(project); // remember it for the import-screen history
      useGlossarySuggest.getState().load(project.root); // restore this game's saved panel
      set({ project, filter: {}, window: EMPTY_WINDOW, total: 0 });
      await get().refreshMeta();
      await get().reloadUnits();
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ loading: false });
    }
  },

  closeProject: async () => {
    await api.closeProject();
    useGlossarySuggest.getState().reset();
    useErrors.getState().reset();
    set({ project: null, files: [], stats: null, total: 0, window: EMPTY_WINDOW });
  },

  setLanguages: async (source, target) => {
    await api.setLanguages(source, target);
    const p = get().project;
    if (p) set({ project: { ...p, sourceLang: source, targetLang: target } });
  },

  setFilter: async (patch) => {
    set({ filter: { ...get().filter, ...patch } });
    await get().reloadUnits();
  },

  // Reset to the first window of the current filter: re-count + fetch window@0.
  reloadUnits: async () => {
    set({ loading: true });
    const req = ++winReq; // invalidate any in-flight ensureWindow (e.g. old filter)
    try {
      const f = get().filter;
      const [total, rows] = await Promise.all([
        api.countUnits(f),
        api.listUnits({ ...f, limit: WINDOW, offset: 0 }),
      ]);
      if (req === winReq) set({ total, window: { offset: 0, rows } });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ loading: false });
    }
  },

  // Full refresh (files + stats) — only after import / translate / apply-TM.
  refreshMeta: async () => {
    const [files, stats] = await Promise.all([api.listFiles(), api.getStats()]);
    set({ files, stats });
  },

  // Stats only — the file list can't change from an edit, so skip that query.
  refreshStats: async () => {
    set({ stats: await api.getStats() });
  },

  refreshTotal: async () => {
    try {
      set({ total: await api.countUnits(get().filter) });
    } catch {
      /* keep the previous total on a transient failure */
    }
  },

  ensureWindow: (start, end) => {
    const { window, total, filter } = get();
    const loStart = window.offset;
    const loEnd = loStart + window.rows.length;
    const needTop = start < loStart || (start - loStart < MARGIN && loStart > 0);
    const needBottom = end >= loEnd || (loEnd - end < MARGIN && loEnd < total);
    if (!needTop && !needBottom) return;
    // Center a WINDOW-sized slice on the visible range, clamped to [0, total).
    const mid = Math.floor((start + end) / 2);
    const offset = Math.max(0, Math.min(mid - Math.floor(WINDOW / 2), Math.max(0, total - WINDOW)));
    if (offset === loStart && window.rows.length > 0) return; // already loaded there
    const req = ++winReq;
    api
      .listUnits({ ...filter, limit: WINDOW, offset })
      .then((rows) => {
        if (req === winReq) set({ window: { offset, rows } });
      })
      .catch(() => {});
  },

  editUnit: async (id, translation, status) => {
    const cur = get().window.rows.find((u) => u.id === id);
    // First edit of an untranslated/failed unit promotes it to Draft.
    const nextStatus: Status =
      status ??
      (cur && (cur.status === "Untranslated" || cur.status === "Failed")
        ? "Draft"
        : cur?.status ?? "Draft");
    await api.updateUnit(id, translation, nextStatus);
    const win = get().window;
    set({
      window: {
        ...win,
        rows: win.rows.map((u) =>
          u.id === id ? { ...u, translation, status: nextStatus } : u
        ),
      },
    });
    await get().refreshStats();
  },

  setStatus: async (id, status) => {
    const cur = get().window.rows.find((u) => u.id === id);
    await api.updateUnit(id, cur?.translation ?? null, status);
    const win = get().window;
    set({
      window: { ...win, rows: win.rows.map((u) => (u.id === id ? { ...u, status } : u)) },
    });
    await get().refreshStats();
  },

  // Apply a batch of Run results the backend already persisted, so the grid fills
  // row-by-row live. Only rows in the loaded window are patched (cheap); rows
  // outside it load correctly the next time they scroll into view.
  applyUnitUpdates: (updates) => {
    if (updates.length === 0) return;
    const byId = new Map(updates.map((u) => [u.id, u]));
    const win = get().window;
    let changed = false;
    const rows = win.rows.map((u) => {
      const up = byId.get(u.id);
      if (up) {
        changed = true;
        return { ...u, translation: up.translation, status: up.status };
      }
      return u;
    });
    if (changed) set({ window: { ...win, rows } });
    void get().refreshStats();
  },
}));
