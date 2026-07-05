import { create } from "zustand";
import type { FailedUnit } from "./ipc";

// Session-only map of unit id → failure reason, filled live from the
// `translate://failed` event. The errors modal uses it to explain each failure;
// the Failed status in the DB stays the source of truth for *which* units failed
// (so a failure from a previous session still lists, just without a reason).
interface ErrorsState {
  byId: Record<number, string>;
  record: (items: FailedUnit[]) => void;
  reset: () => void;
}

export const useErrors = create<ErrorsState>((set) => ({
  byId: {},
  record: (items) =>
    set((s) => {
      if (items.length === 0) return s;
      const byId = { ...s.byId };
      for (const it of items) byId[it.id] = it.reason;
      return { byId };
    }),
  reset: () => set({ byId: {} }),
}));
