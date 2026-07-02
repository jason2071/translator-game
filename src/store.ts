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

interface AppStore {
  project: ProjectInfo | null;
  files: FileCount[];
  stats: Stats | null;
  units: TransUnit[];
  filter: UnitFilter;
  loading: boolean;
  error: string | null;

  openProject: (path: string) => Promise<void>;
  closeProject: () => Promise<void>;
  setFilter: (patch: Partial<UnitFilter>) => Promise<void>;
  reloadUnits: () => Promise<void>;
  refreshMeta: () => Promise<void>;
  editUnit: (id: number, translation: string, status?: Status) => Promise<void>;
  setStatus: (id: number, status: Status) => Promise<void>;
}

const PAGE = 2000;

export const useStore = create<AppStore>((set, get) => ({
  project: null,
  files: [],
  stats: null,
  units: [],
  filter: { limit: PAGE, offset: 0 },
  loading: false,
  error: null,

  openProject: async (path) => {
    set({ loading: true, error: null });
    try {
      const project = await api.openProject(path);
      set({ project, filter: { limit: PAGE, offset: 0 } });
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
    set({ project: null, files: [], stats: null, units: [] });
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

  refreshMeta: async () => {
    const [files, stats] = await Promise.all([api.listFiles(), api.getStats()]);
    set({ files, stats });
  },

  editUnit: async (id, translation, status) => {
    const units = get().units;
    const cur = units.find((u) => u.id === id);
    // First edit of an untranslated unit promotes it to Draft.
    const nextStatus: Status =
      status ??
      (cur && cur.status === "Untranslated" ? "Draft" : cur?.status ?? "Draft");
    await api.updateUnit(id, translation, nextStatus);
    set({
      units: units.map((u) =>
        u.id === id ? { ...u, translation, status: nextStatus } : u
      ),
    });
    await get().refreshMeta();
  },

  setStatus: async (id, status) => {
    const cur = get().units.find((u) => u.id === id);
    await api.updateUnit(id, cur?.translation ?? null, status);
    set({
      units: get().units.map((u) => (u.id === id ? { ...u, status } : u)),
    });
    await get().refreshMeta();
  },
}));
