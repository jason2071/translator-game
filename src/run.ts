import { create } from "zustand";
import { api, type TranslateScope, type TranslateSummary } from "./ipc";
import { useTranslation } from "./translation";
import { useSettings } from "./settings";
import { useStore } from "./store";

// The last units-run: how it was started and how it ended. Any surface can
// start a run (the toolbar's Translate, the failure chip's Retry failed, the
// filter bar's Re-translate); the status row in TranslateBar renders whatever
// this store holds, so the outcome shows regardless of which button fired.
interface RunState {
  summary: TranslateSummary | null;
  err: string | null;
  run: (scope: TranslateScope) => Promise<void>;
}

export const useRun = create<RunState>((set) => ({
  summary: null,
  err: null,
  run: async (scope) => {
    set({ summary: null, err: null });
    try {
      const res = await useTranslation
        .getState()
        .enqueue("units", () =>
          api.translateUnits(scope, useSettings.getState().activeConfig())
        );
      set({ summary: res });
      // The visible rows were live-patched during the Run; just refresh the
      // sidebar counts and the total (no full reload → no scroll jump).
      await useStore.getState().refreshMeta();
      await useStore.getState().refreshTotal();
    } catch (e) {
      set({ err: String(e) });
    }
  },
}));
