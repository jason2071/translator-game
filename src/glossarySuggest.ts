import { create } from "zustand";
import { api, type GlossCandidate, type ProviderConfig } from "./ipc";
import { useTranslation } from "./translation";

// Glossary auto-suggest state lives in a store (not the modal component) so
// closing the modal never loses candidates, filled translations, or an
// in-flight AI translate — the backend keeps running and results land here.
// The AI fill goes through the shared `useTranslation.run` so it shares the
// one progress bar with the main Run and the two never overlap.

interface Row {
  on: boolean;
  tr: string;
}

interface SuggestState {
  cands: GlossCandidate[] | null;
  rows: Record<string, Row>;
  loading: boolean; // scanning for candidates
  msg: string | null;

  suggest: () => Promise<void>;
  translateEmpty: (cfg: ProviderConfig) => Promise<void>;
  setRow: (term: string, patch: Partial<Row>) => void;
  addSelected: (onAdded: () => void) => Promise<void>;
  clear: () => void; // back to the suggest button, keep nothing
  reset: () => void; // full reset (on project change)
}

export const useGlossarySuggest = create<SuggestState>((set, get) => ({
  cands: null,
  rows: {},
  loading: false,
  msg: null,

  suggest: async () => {
    set({ loading: true, msg: null });
    try {
      const c = await api.suggestGlossary();
      const rows: Record<string, Row> = {};
      for (const x of c) rows[x.term] = { on: true, tr: x.translation ?? "" };
      set({ cands: c, rows });
    } catch (e) {
      set({ msg: String(e) });
    } finally {
      set({ loading: false });
    }
  },

  translateEmpty: async (cfg) => {
    const { cands, rows } = get();
    if (!cands) return;
    const todo = cands.filter((c) => !(rows[c.term]?.tr ?? "").trim());
    if (todo.length === 0) return;
    if (useTranslation.getState().busy) {
      set({ msg: "Another translation is running" });
      return;
    }
    set({ msg: null });
    // Fill each row live as its term returns, so the user watches which are
    // done instead of the whole batch appearing at the end. The listener lives
    // here (not the component), so results keep landing even if the modal closes.
    const unlisten = await api.onTextItem((it) => {
      const term = todo[it.index]?.term;
      if (term && it.text) {
        set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], tr: it.text! } } }));
      }
    });
    try {
      const res = await useTranslation
        .getState()
        .run("glossary", () => api.translateTexts(todo.map((c) => c.term), cfg));
      // Rows already filled live; here just tally for the summary. A null result
      // = that term failed (or the run was cancelled).
      let filled = 0;
      let failed = 0;
      const pairs: [string, string][] = [];
      todo.forEach((c, i) => {
        const t = res[i];
        if (t) {
          filled++;
          pairs.push([c.term, t]);
        } else {
          failed++;
        }
      });
      // Persist to TM so re-suggesting prefills these instead of re-translating.
      if (pairs.length) await api.rememberTexts(pairs);
      set({ msg: failed > 0 ? `Filled ${filled} · ${failed} failed` : `Filled ${filled}` });
    } catch (e) {
      set({ msg: String(e) });
    } finally {
      unlisten();
    }
  },

  setRow: (term, patch) =>
    set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], ...patch } } })),

  addSelected: async (onAdded) => {
    const { cands, rows } = get();
    if (!cands) return;
    const items: [string, string][] = cands
      .filter((c) => rows[c.term]?.on && rows[c.term]?.tr.trim())
      .map((c) => [c.term, rows[c.term].tr.trim()]);
    const n = await api.glossaryAddBulk(items);
    set({ cands: null, rows: {}, msg: `Added ${n}` });
    onAdded();
  },

  clear: () => set({ cands: null, rows: {} }),
  reset: () => set({ cands: null, rows: {}, loading: false, msg: null }),
}));
