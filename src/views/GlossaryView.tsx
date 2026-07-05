import { useEffect, useState } from "react";
import { api, type GlossaryEntry, type ProviderKind } from "../ipc";
import { PROVIDER_LABELS, PROVIDER_KINDS, useSettings } from "../settings";
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

function SuggestPanel({ onAdded }: { onAdded: () => void }) {
  const glossaryConfig = useSettings((s) => s.glossaryConfig);
  const glossaryProvider = useSettings((s) => s.glossaryProvider);
  const setGlossaryProvider = useSettings((s) => s.setGlossaryProvider);
  const {
    cands,
    rows,
    loading,
    msg,
    suggest,
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

  const filled = cands
    ? cands.filter((c) => (rows[c.term]?.tr ?? "").trim()).length
    : 0;
  // Remaining = failed + never-translated + skipped; the AI button retries these.
  const remaining = cands ? cands.length - filled : 0;

  if (!cands) {
    return (
      <div className="suggest-bar">
        <button className="ghost" onClick={suggest} disabled={loading}>
          <Icon name="sparkle" size={14} /> {loading ? "Scanning…" : "Auto-suggest from game"}
        </button>
        {translating && <span className="hint">Translating in background…</span>}
        {msg && <span className="ok-msg">{msg}</span>}
      </div>
    );
  }

  return (
    <div className="suggest-panel">
      <div className="suggest-head">
        <strong>
          {cands.length} candidates
          <span className="hint"> · {filled} filled</span>
        </strong>
        {msg && (
          <span className={/fail|error|running|no api/i.test(msg) ? "error" : "ok-msg"}>
            {msg}
          </span>
        )}
        <select
          className="gloss-provider"
          value={glossaryProvider}
          onChange={(e) => setGlossaryProvider(e.target.value as ProviderKind)}
          disabled={glossBusy}
          title="AI provider used for glossary translation (independent of the Run provider)"
        >
          {PROVIDER_KINDS.map((k) => (
            <option key={k} value={k}>
              {PROVIDER_LABELS[k]}
            </option>
          ))}
        </select>
        {glossBusy ? (
          <button className="ghost" onClick={() => cancel("glossary")}>
            {translating ? "Cancel translate" : "Cancel queued"}
          </button>
        ) : (
          <>
            <button
              className="ghost"
              onClick={() => translateEmpty(glossaryConfig())}
              disabled={remaining === 0}
              title={
                remaining === 0
                  ? "All candidates have a translation"
                  : "Translate every empty/failed/skipped term (queues behind a running Run)"
              }
            >
              <Icon name={filled > 0 ? "retry" : "globe"} size={14} />{" "}
              {filled > 0
                ? `Re-translate remaining (${remaining})`
                : `Translate empty (${remaining})`}
            </button>
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
        <button className="primary" onClick={() => addSelected(onAdded)} disabled={glossBusy}>
          Add selected
        </button>
        <button className="ghost" onClick={clear} disabled={glossBusy}>
          Cancel
        </button>
      </div>
      {/* Modal shows only the glossary status. */}
      <TransProgress kind="glossary" />
      {translating && (
        <div className="hint suggest-note">
          Running in background — safe to close this dialog; results are kept.
        </div>
      )}
      <div className="suggest-list">
        {cands.map((c) => {
          const done = !!(rows[c.term]?.tr ?? "").trim();
          return (
            <div key={c.term} className={`suggest-row${done ? " filled" : ""}`}>
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
                {done && <span className="cand-mark">✓</span>}
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
