import { useEffect, useState } from "react";
import { api, type GlossaryEntry } from "../ipc";
import { useSettings } from "../settings";
import { useGlossarySuggest } from "../glossarySuggest";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";

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
  const activeConfig = useSettings((s) => s.activeConfig);
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

  // Shared status with the main Run: translating here == a glossary run is live.
  const busy = useTranslation((s) => s.busy);
  const kind = useTranslation((s) => s.kind);
  const cancel = useTranslation((s) => s.cancel);
  const translating = busy && kind === "glossary";
  const otherBusy = busy && kind !== "glossary"; // a unit Run is holding the lock

  const filled = cands
    ? cands.filter((c) => (rows[c.term]?.tr ?? "").trim()).length
    : 0;
  // Remaining = failed + never-translated + skipped; the AI button retries these.
  const remaining = cands ? cands.length - filled : 0;

  if (!cands) {
    return (
      <div className="suggest-bar">
        <button className="ghost" onClick={suggest} disabled={loading}>
          {loading ? "Scanning…" : "✨ Auto-suggest from game"}
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
        {translating ? (
          <button className="ghost" onClick={cancel}>
            Cancel translate
          </button>
        ) : (
          <button
            className="ghost"
            onClick={() => translateEmpty(activeConfig())}
            disabled={otherBusy || remaining === 0}
            title={
              otherBusy
                ? "A translation is already running"
                : remaining === 0
                  ? "All candidates have a translation"
                  : "Translate every empty/failed/skipped term"
            }
          >
            {filled > 0
              ? `↻ Re-translate remaining (${remaining})`
              : `🌐 Translate empty (${remaining})`}
          </button>
        )}
        <button className="primary" onClick={() => addSelected(onAdded)} disabled={translating}>
          Add selected
        </button>
        <button className="ghost" onClick={clear} disabled={translating}>
          Cancel
        </button>
      </div>
      <TransProgress only="glossary" />
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
              {done ? (
                <span className="cand-mark">✓</span>
              ) : (
                <button
                  className="cand-retry"
                  onClick={() => translateOne(c.term, activeConfig())}
                  disabled={busy}
                  title="Translate this term with AI"
                >
                  ↻
                </button>
              )}
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
          className="ghost"
          onClick={async () => {
            await api.glossaryDelete(entry.id);
            onChanged();
          }}
        >
          🗑
        </button>
      </td>
    </tr>
  );
}
