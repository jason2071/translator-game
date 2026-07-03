import { create } from "zustand";
import {
  api,
  type FileCount,
  type ProjectInfo,
  type Stats,
  type Status,
  type TransUnit,
  type UnitFilter,
} from "./ipc";
import { useGlossarySuggest } from "./glossarySuggest";

interface AppStore {
  project: ProjectInfo | null;
  files: FileCount[];
  stats: Stats | null;
  units: TransUnit[];
  filter: UnitFilter;
  loading: boolean;
  error: string | null;

  openProject: (path: string, sourceLang?: string, targetLang?: string) => Promise<void>;
  reextract: () => Promise<void>;
  closeProject: () => Promise<void>;
  setLanguages: (source: string, target: string) => Promise<void>;
  setFilter: (patch: Partial<UnitFilter>) => Promise<void>;
  reloadUnits: () => Promise<void>;
  refreshMeta: () => Promise<void>;
  refreshStats: () => Promise<void>;
  editUnit: (id: number, translation: string, status?: Status) => Promise<void>;
  setStatus: (id: number, status: Status) => Promise<void>;
}

// Load the whole matching set; the grid is virtualized so only the visible
// rows mount, and browsing a 13k-unit game stays smooth.
const PAGE = 100000;

export const useStore = create<AppStore>((set, get) => ({
  project: null,
  files: [],
  stats: null,
  units: [],
  filter: { limit: PAGE, offset: 0 },
  loading: false,
  error: null,

  openProject: async (path, sourceLang, targetLang) => {
    set({ loading: true, error: null });
    try {
      const project = await api.openProject(path, sourceLang, targetLang);
      useGlossarySuggest.getState().load(project.root); // restore this game's saved panel
      set({ project, filter: { limit: PAGE, offset: 0 } });
      await get().refreshMeta();
      await get().reloadUnits();
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ loading: false });
    }
  },

  reextract: async () => {
    set({ loading: true, error: null });
    try {
      await api.reextract();
      set({ filter: { limit: PAGE, offset: 0 } });
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
    set({ project: null, files: [], stats: null, units: [] });
  },

  setLanguages: async (source, target) => {
    await api.setLanguages(source, target);
    const p = get().project;
    if (p) set({ project: { ...p, sourceLang: source, targetLang: target } });
  },

  setFilter: async (patch) => {
    set({ filter: { ...get().filter, ...patch, offset: 0 } });
    await get().reloadUnits();
  },

  reloadUnits: async () => {
    set({ loading: true });
    try {
      const units = await api.listUnits(get().filter);
      set({ units });
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

  editUnit: async (id, translation, status) => {
    const units = get().units;
    const cur = units.find((u) => u.id === id);
    // First edit of an untranslated/failed unit promotes it to Draft.
    const nextStatus: Status =
      status ??
      (cur && (cur.status === "Untranslated" || cur.status === "Failed")
        ? "Draft"
        : cur?.status ?? "Draft");
    await api.updateUnit(id, translation, nextStatus);
    set({
      units: units.map((u) =>
        u.id === id ? { ...u, translation, status: nextStatus } : u
      ),
    });
    await get().refreshStats();
  },

  setStatus: async (id, status) => {
    const cur = get().units.find((u) => u.id === id);
    await api.updateUnit(id, cur?.translation ?? null, status);
    set({
      units: get().units.map((u) => (u.id === id ? { ...u, status } : u)),
    });
    await get().refreshStats();
  },
}));
