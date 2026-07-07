import { create } from "zustand";
import { api, type GlossCandidate, type ProviderConfig } from "./ipc";
import { useTranslation } from "./translation";

// Glossary auto-suggest state lives in a store (not the modal component) so
// closing the modal never loses candidates, filled translations, or an
// in-flight AI translate. It is ALSO persisted to localStorage per game, so
// closing and reopening the whole app restores the panel exactly as left —
// every translation (AI or hand-typed) survives. The AI fill goes through the
// shared `useTranslation.run` so it shares the one progress bar with the main
// Run and the two never overlap. AI results are additionally written to TM
// (remember_texts) so they dedup against unit translation too.

interface Row {
  on: boolean;
  tr: string;
  /** The last AI attempt for this term failed (empty/mangled) — kept so the UI
   *  can flag it even when an older translation is still in `tr`. */
  failed?: boolean;
}

interface SuggestState {
  root: string | null; // current game, used as the localStorage key
  cands: GlossCandidate[] | null;
  rows: Record<string, Row>;
  loading: boolean; // scanning for candidates
  adding: boolean; // an "Add selected" write is in flight (guards double-click)
  msg: string | null;
  suggestStage: string | null; // live phase text during an AI suggest (scan → ask)

  load: (root: string) => void; // restore this game's saved panel (on open)
  suggest: () => Promise<void>;
  // AI-mine candidates from the game's dialogue/text (catches proper nouns the
  // heuristic misses). Merges into any candidates already present.
  suggestAi: (cfg: ProviderConfig) => Promise<void>;
  // Translate candidates via AI. By default only the empty/failed/skipped ones;
  // pass all=true to re-translate every candidate, overwriting filled rows.
  translateEmpty: (cfg: ProviderConfig, all?: boolean) => Promise<void>;
  translateOne: (term: string, cfg: ProviderConfig) => Promise<void>; // retry one
  setRow: (term: string, patch: Partial<Row>) => void;
  addSelected: (onAdded: () => void) => Promise<void>;
  clear: () => void; // discard the working set (also on disk)
  reset: () => void; // drop in-memory state only, keep disk (on project close)
}

const keyFor = (root: string) => `rpgtl.gsuggest.${root}`;

export const useGlossarySuggest = create<SuggestState>((set, get) => {
  // Write the working set to disk (or clear it when there are no candidates).
  const persist = () => {
    const { root, cands, rows } = get();
    if (!root) return;
    try {
      if (cands) localStorage.setItem(keyFor(root), JSON.stringify({ cands, rows }));
      else localStorage.removeItem(keyFor(root));
    } catch {
      /* quota / disabled storage — non-fatal */
    }
  };

  return {
    root: null,
    cands: null,
    rows: {},
    loading: false,
    adding: false,
    msg: null,
    suggestStage: null,

    load: (root) => {
      let cands: GlossCandidate[] | null = null;
      let rows: Record<string, Row> = {};
      try {
        const raw = localStorage.getItem(keyFor(root));
        if (raw) {
          const d = JSON.parse(raw);
          cands = d.cands ?? null;
          rows = d.rows ?? {};
        }
      } catch {
        /* corrupt entry — start fresh */
      }
      set({ root, cands, rows, loading: false, msg: null, suggestStage: null });
    },

    suggest: async () => {
      set({ loading: true, msg: null });
      try {
        const c = await api.suggestGlossary();
        // Merge: keep every existing (possibly hand-edited) translation, fill
        // empties from the DB/TM prefill, and add any new candidates.
        set((s) => {
          const rows = { ...s.rows };
          for (const x of c) {
            if (!rows[x.term]) rows[x.term] = { on: true, tr: x.translation ?? "" };
            else if (!(rows[x.term].tr ?? "").trim() && x.translation)
              rows[x.term] = { ...rows[x.term], tr: x.translation };
          }
          return { cands: c, rows };
        });
        persist();
      } catch (e) {
        set({ msg: String(e) });
      } finally {
        set({ loading: false });
      }
    },

    suggestAi: async (cfg) => {
      set({ loading: true, msg: null, suggestStage: "Scanning game…" });
      // Follow the backend's phase events so the button shows real progress (the
      // whole-game scan, then the AI wait) instead of a silent spinner.
      const unlisten = await api.onGlossarySuggest((s) => {
        set({
          suggestStage:
            s.stage === "asking"
              ? `Asking AI · ${s.count} term${s.count === 1 ? "" : "s"}…`
              : "Scanning game…",
        });
      });
      try {
        const ai = await api.suggestGlossaryAi(cfg);
        // Merge with any candidates already present (a prior heuristic run),
        // dedup by term; fill empty rows from the AI's suggested translation.
        set((s) => {
          const byTerm = new Map<string, GlossCandidate>();
          for (const c of s.cands ?? []) byTerm.set(c.term, c);
          for (const c of ai) if (!byTerm.has(c.term)) byTerm.set(c.term, c);
          const rows = { ...s.rows };
          for (const x of ai) {
            if (!rows[x.term]) rows[x.term] = { on: true, tr: x.translation ?? "" };
            else if (!(rows[x.term].tr ?? "").trim() && x.translation)
              rows[x.term] = { ...rows[x.term], tr: x.translation };
          }
          return { cands: [...byTerm.values()], rows };
        });
        persist();
        set({ msg: ai.length ? `AI found ${ai.length} term(s)` : "AI found no new terms" });
      } catch (e) {
        set({ msg: String(e) });
      } finally {
        unlisten();
        set({ loading: false, suggestStage: null });
      }
    },

    translateEmpty: async (cfg, all = false) => {
      const { cands, rows } = get();
      if (!cands) return;
      // Empty OR previously-failed terms (re-translate-all takes everything).
      const todo = all
        ? cands
        : cands.filter((c) => !(rows[c.term]?.tr ?? "").trim() || rows[c.term]?.failed);
      if (todo.length === 0) return;
      if (useTranslation.getState().glossary.phase !== "idle") {
        set({ msg: "Glossary translate already running" });
        return;
      }
      set({ msg: null });
      // Fill each row live as its term returns, so the user watches which are
      // done instead of the whole batch appearing at the end. The listener lives
      // here (not the component), so results keep landing even if the modal closes.
      const unlisten = await api.onTextItem((it) => {
        const term = todo[it.index]?.term;
        if (term && it.text) {
          set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], tr: it.text!, failed: false } } }));
          persist();
        }
      });
      try {
        const res = await useTranslation
          .getState()
          .enqueue("glossary", () => api.translateTexts(todo.map((c) => c.term), cfg));
        // Rows already filled live; here tally, remember, and flag the failures so
        // the UI can surface exactly which terms didn't translate. A null result
        // = that term failed (or the run was cancelled).
        let filled = 0;
        let failed = 0;
        const pairs: [string, string][] = [];
        set((s) => {
          const next = { ...s.rows };
          todo.forEach((c, i) => {
            const t = res[i];
            if (t) {
              filled++;
              pairs.push([c.term, t]);
              next[c.term] = { ...next[c.term], failed: false };
            } else {
              failed++;
              next[c.term] = { ...next[c.term], failed: true };
            }
          });
          return { rows: next };
        });
        // Persist to TM (dedup vs unit translation) and to disk (panel survives).
        if (pairs.length) await api.rememberTexts(pairs);
        persist();
        set({ msg: failed > 0 ? `${failed} term(s) failed — see the failed filter` : null });
      } catch (e) {
        set({ msg: String(e) });
      } finally {
        unlisten();
      }
    },

    translateOne: async (term, cfg) => {
      if (useTranslation.getState().glossary.phase !== "idle") {
        set({ msg: "Glossary translate already running" });
        return;
      }
      set({ msg: null });
      try {
        const res = await useTranslation
          .getState()
          .enqueue("glossary", () => api.translateTexts([term], cfg));
        const t = res[0];
        if (t) {
          set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], tr: t, failed: false } } }));
          await api.rememberTexts([[term, t]]);
          persist();
          set({ msg: null });
        } else {
          set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], failed: true } } }));
          set({ msg: `${term} failed` });
        }
      } catch (e) {
        set({ msg: String(e) });
      }
    },

    setRow: (term, patch) => {
      set((s) => ({ rows: { ...s.rows, [term]: { ...s.rows[term], ...patch } } }));
      persist();
    },

    addSelected: async (onAdded) => {
      const { cands, rows, adding } = get();
      if (!cands || adding) return; // guard a double-click while the write is in flight
      const items: [string, string][] = cands
        .filter((c) => rows[c.term]?.on && rows[c.term]?.tr.trim())
        .map((c) => [c.term, rows[c.term].tr.trim()]);
      set({ adding: true });
      try {
        const n = await api.glossaryAddBulk(items);
        set({ cands: null, rows: {}, msg: `Added ${n}` });
        persist(); // cands null -> removes the saved working set
        onAdded();
      } catch (e) {
        set({ msg: String(e) });
      } finally {
        set({ adding: false });
      }
    },

    clear: () => {
      set({ cands: null, rows: {} });
      persist();
    },
    reset: () => set({ root: null, cands: null, rows: {}, loading: false, msg: null, suggestStage: null }),
  };
});
