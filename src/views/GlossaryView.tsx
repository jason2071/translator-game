import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, type GlossaryEntry, type ProviderKind } from "../ipc";
import { PROVIDER_LABELS, PROVIDER_KINDS, useSettings } from "../settings";
import { useStore } from "../store";
import { useGlossarySuggest } from "../glossarySuggest";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";

export default function GlossaryView() {
  const [entries, setEntries] = useState<GlossaryEntry[]>([]);
  const [term, setTerm] = useState("");
  const [translation, setTranslation] = useState("");
  const [note, setNote] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);

  async function reload() {
    setEntries(await api.glossaryList());
  }
  useEffect(() => {
    reload();
  }, []);

  async function add() {
    if (!term.trim() || !translation.trim()) return;
    await api.glossaryAdd(term.trim(), translation.trim(), note.trim() || undefined, caseSensitive);
    setTerm("");
    setTranslation("");
    setNote("");
    setCaseSensitive(false);
    await reload();
  }

  return (
    <div className="glossary">
      <GameContextPanel />

      <p className="hint">
        Terms are fed to the AI and used to lint translations for consistency
        (proper nouns, stats, item names).
      </p>

      <SuggestPanel onAdded={reload} />


      <div className="gloss-add">
        <input placeholder="Source term" value={term} onChange={(e) => setTerm(e.target.value)} />
        <input
          placeholder="Translation"
          value={translation}
          onChange={(e) => setTranslation(e.target.value)}
        />
        <input placeholder="Note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
        <label className="chk">
          <input
            type="checkbox"
            checked={caseSensitive}
            onChange={(e) => setCaseSensitive(e.target.checked)}
          />
          Aa
        </label>
        <button className="primary" onClick={add}>
          Add
        </button>
      </div>

      <table className="gloss-table">
        <thead>
          <tr>
            <th>Term</th>
            <th>Translation</th>
            <th>Note</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {entries.map((g) => (
            <GlossRow key={g.id} entry={g} onChanged={reload} />
          ))}
          {entries.length === 0 && (
            <tr>
              <td colSpan={4} className="empty">
                No glossary entries yet.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

// Setting-era presets. The value is the key the backend maps to a register
// directive (see `ai::prompt::era_directive`); "" = none (rely on game context).
const ERA_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Era: none" },
  { value: "ancient", label: "Ancient / Epic (โบราณ)" },
  { value: "medieval", label: "Medieval / Fantasy (ยุคกลาง)" },
  { value: "wuxia", label: "Wuxia / Xianxia (กำลังภายใน)" },
  { value: "samurai", label: "Feudal Japan (ซามูไร)" },
  { value: "modern", label: "Modern (ปัจจุบัน)" },
  { value: "scifi", label: "Sci-fi / Future (ไซไฟ)" },
];

// Per-project game context (lore/setting) — stored in the project DB and fed to
// the model on every Run. Lives here (per-project, like the glossary) rather than
// in Settings (which is global/per-provider). "AI draft" fills it from the game.
// The Era dropdown is a shortcut that seeds period-appropriate register/pronouns
// into the prompt, composing with (and framing) the free-text context below.
function GameContextPanel() {
  const project = useStore((s) => s.project);
  const setGameContext = useStore((s) => s.setGameContext);
  const setEra = useStore((s) => s.setEra);
  const glossaryConfig = useSettings((s) => s.glossaryConfig);
  // The AI provider here governs BOTH "AI draft" (context) and the glossary's
  // "AI suggest" — surfaced at the top so it's visible before either is run.
  const glossaryProvider = useSettings((s) => s.glossaryProvider);
  const setGlossaryProvider = useSettings((s) => s.setGlossaryProvider);
  const [drafting, setDrafting] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  if (!project) return null;

  async function draft() {
    const p = useStore.getState().project;
    if (!p) return;
    if (p.gameContext.trim()) {
      const ok = await ask("Replace the current game context with an AI draft from the game's text?", {
        title: "AI draft game context?",
        kind: "warning",
      });
      if (!ok) return;
    }
    setDrafting(true);
    setMsg(null);
    try {
      const text = (await api.suggestGameContext(glossaryConfig())).trim();
      if (text) {
        setGameContext(text);
        setMsg("Drafted from the game's text — edit as needed.");
      } else {
        setMsg("No context could be drafted (no sampled text).");
      }
    } catch (e) {
      setMsg(String(e));
    } finally {
      setDrafting(false);
    }
  }

  return (
    <div className="gloss-context">
      <div className="gloss-context-head">
        <label>
          Game context <span className="hint">(this project)</span>
        </label>
        <div className="gloss-context-actions">
          <select
            className="gloss-provider"
            value={project.era ?? ""}
            onChange={(e) => setEra(e.target.value)}
            title="Setting era — seeds period-appropriate register/pronouns (e.g. ancient → ข้า/เจ้า) into the AI prompt on every Run"
          >
            {ERA_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <select
            className="gloss-provider"
            value={glossaryProvider}
            onChange={(e) => setGlossaryProvider(e.target.value as ProviderKind)}
            disabled={drafting}
            title="AI provider for glossary + game-context help (independent of the Run provider)"
          >
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {PROVIDER_LABELS[k]}
              </option>
            ))}
          </select>
          <button
            className="ghost"
            onClick={draft}
            disabled={drafting}
            title="Draft a setting/character brief from this game's own text with AI"
          >
            <Icon name="sparkle" size={14} /> {drafting ? "Drafting…" : "AI draft"}
          </button>
        </div>
      </div>
      <textarea
        rows={3}
        placeholder="Lore/setting for THIS game — era, characters, relationships, tone, world rules. Fed to the AI on every Run. e.g. Modern-day Thailand; Callum and Daisy are siblings; casual speech."
        value={project.gameContext}
        onChange={(e) => setGameContext(e.target.value)}
      />
      {msg && <span className={/fail|error|no api/i.test(msg) ? "error" : "ok-msg"}>{msg}</span>}
    </div>
  );
}

function SuggestPanel({ onAdded }: { onAdded: () => void }) {
  const glossaryConfig = useSettings((s) => s.glossaryConfig);
  const {
    cands,
    rows,
    loading,
    adding,
    msg,
    suggestStage,
    suggest,
    suggestAi,
    translateEmpty,
    translateOne,
    setRow,
    addSelected,
    clear,
  } = useGlossarySuggest();

  // Glossary's own queue slot: running (live) or queued (waiting behind a Run).
  const phase = useTranslation((s) => s.glossary.phase);
  const cancel = useTranslation((s) => s.cancel);
  const translating = phase === "running";
  const glossBusy = phase !== "idle"; // running or queued — can't queue twice

  const failedCount = cands ? cands.filter((c) => rows[c.term]?.failed).length : 0;
  // Filled = has a translation and the last attempt didn't fail.
  const filled = cands
    ? cands.filter((c) => (rows[c.term]?.tr ?? "").trim() && !rows[c.term]?.failed).length
    : 0;
  // Remaining = empty + failed; the AI button retries these.
  const remaining = cands ? cands.length - filled : 0;

  // Filter the candidate list down to just the failures, so they're easy to find.
  const [failedOnly, setFailedOnly] = useState(false);
  useEffect(() => {
    if (failedCount === 0) setFailedOnly(false);
  }, [failedCount]);

  if (!cands) {
    return (
      <div className="suggest-bar">
        <button className="ghost" onClick={suggest} disabled={loading}>
          <Icon name="sparkle" size={14} /> {loading ? "Scanning…" : "Auto-suggest from game"}
        </button>
        <button
          className="ghost"
          onClick={() => suggestAi(glossaryConfig())}
          disabled={loading}
          title="Use AI to mine proper nouns/terms from the game's dialogue (catches names the heuristic misses)"
        >
          <Icon name="sparkle" size={14} /> {suggestStage ?? "AI suggest"}
        </button>
        {translating && <span className="hint">Translating in background…</span>}
        {msg && <span className={/fail|error|no api/i.test(msg) ? "error" : "ok-msg"}>{msg}</span>}
      </div>
    );
  }

  return (
    <div className="suggest-panel">
      <div className="suggest-head">
        <div className="suggest-head-info">
          <strong>{cands.length} candidates</strong>
          <span className="hint">· {filled} filled</span>
          {failedCount > 0 && (
            <button
              type="button"
              className={`gloss-failed-toggle${failedOnly ? " active" : ""}`}
              onClick={() => setFailedOnly((v) => !v)}
              title={failedOnly ? "Show all terms" : "Show only the terms that failed"}
            >
              · {failedCount} failed
            </button>
          )}
          {msg && (
            <span className={/fail|error|running|no api/i.test(msg) ? "error" : "ok-msg"}>
              {msg}
            </span>
          )}
        </div>

        <div className="suggest-head-actions">
          <button
            className="ghost"
            onClick={() => suggestAi(glossaryConfig())}
            disabled={loading || glossBusy}
            title="Mine more terms from the game's dialogue with AI (provider chosen at the top)"
          >
            <Icon name="sparkle" size={14} />{" "}
            {suggestStage ?? (loading ? "AI scanning…" : "AI suggest")}
          </button>

          {glossBusy ? (
            <button className="ghost" onClick={() => cancel("glossary")}>
              {translating ? "Cancel translate" : "Cancel queued"}
            </button>
          ) : (
            <>
              {remaining > 0 && (
                <button
                  className="ghost"
                  onClick={() => translateEmpty(glossaryConfig())}
                  title="Translate every empty/failed/skipped term (queues behind a running Run)"
                >
                  <Icon name={filled > 0 ? "retry" : "globe"} size={14} />{" "}
                  {filled > 0 ? `Remaining (${remaining})` : `Translate empty (${remaining})`}
                </button>
              )}
              {filled > 0 && (
                <button
                  className="ghost"
                  onClick={() => translateEmpty(glossaryConfig(), true)}
                  title="Re-translate every term with AI, overwriting ones already filled"
                >
                  <Icon name="retry" size={14} /> Re-translate all ({cands.length})
                </button>
              )}
            </>
          )}

          <span className="suggest-head-sep" />
          <button
            className="primary"
            onClick={() => addSelected(onAdded)}
            disabled={glossBusy || adding}
          >
            {adding ? "Adding…" : "Add selected"}
          </button>
          <button className="ghost" onClick={clear} disabled={glossBusy}>
            Cancel
          </button>
        </div>
      </div>
      {/* One tidy progress block: the bar + count, then the background note. */}
      {glossBusy && (
        <div className="suggest-progress">
          <TransProgress kind="glossary" />
          {translating && (
            <p className="suggest-note">
              Running in background — safe to close this dialog; results are kept.
            </p>
          )}
        </div>
      )}
      <div className="suggest-list">
        {cands.map((c) => {
          const failed = !!rows[c.term]?.failed;
          const done = !!(rows[c.term]?.tr ?? "").trim() && !failed;
          if (failedOnly && !failed) return null;
          return (
            <div
              key={c.term}
              className={`suggest-row${failed ? " failed" : done ? " filled" : ""}`}
            >
              <input
                type="checkbox"
                checked={rows[c.term]?.on ?? true}
                onChange={(e) => setRow(c.term, { on: e.target.checked })}
              />
              <span className="cand-term" title={`${c.kind} ×${c.count}`}>
                {c.term}
              </span>
              <input
                className="cand-tr"
                placeholder="translation…"
                value={rows[c.term]?.tr ?? ""}
                onChange={(e) => setRow(c.term, { tr: e.target.value })}
              />
              <span className="cand-actions">
                {failed ? (
                  <span className="cand-fail" title="AI failed for this term — retry it">
                    ⚠
                  </span>
                ) : (
                  done && <span className="cand-mark">✓</span>
                )}
                <button
                  className="cand-retry iconbtn"
                  onClick={() => translateOne(c.term, glossaryConfig())}
                  disabled={glossBusy}
                  aria-label={`${done ? "Re-translate" : "Translate"} ${c.term} with AI`}
                  title={done ? "Re-translate this term with AI" : "Translate this term with AI"}
                >
                  <Icon name="retry" size={14} />
                </button>
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function GlossRow({ entry, onChanged }: { entry: GlossaryEntry; onChanged: () => void }) {
  const [term, setTerm] = useState(entry.term);
  const [translation, setTranslation] = useState(entry.translation);
  const [note, setNote] = useState(entry.note ?? "");

  async function save() {
    if (term === entry.term && translation === entry.translation && (note || null) === entry.note)
      return;
    await api.glossaryUpdate(entry.id, term, translation, note || undefined, entry.caseSensitive);
    onChanged();
  }

  return (
    <tr>
      <td>
        <input value={term} onChange={(e) => setTerm(e.target.value)} onBlur={save} />
      </td>
      <td>
        <input value={translation} onChange={(e) => setTranslation(e.target.value)} onBlur={save} />
      </td>
      <td>
        <input value={note} onChange={(e) => setNote(e.target.value)} onBlur={save} />
      </td>
      <td>
        <button
          className="iconbtn"
          aria-label={`Delete glossary term ${entry.term}`}
          title="Delete"
          onClick={async () => {
            await api.glossaryDelete(entry.id);
            onChanged();
          }}
        >
          <Icon name="trash" size={15} />
        </button>
      </td>
    </tr>
  );
}
