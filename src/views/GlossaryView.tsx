import { useEffect, useState } from "react";
import { api, type GlossaryEntry, type GlossCandidate } from "../ipc";
import { useSettings } from "../settings";

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
  const [cands, setCands] = useState<GlossCandidate[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [translating, setTranslating] = useState(false);
  const [rows, setRows] = useState<Record<string, { on: boolean; tr: string }>>({});
  const [msg, setMsg] = useState<string | null>(null);

  async function suggest() {
    setLoading(true);
    setMsg(null);
    try {
      const c = await api.suggestGlossary();
      setCands(c);
      const init: Record<string, { on: boolean; tr: string }> = {};
      for (const x of c) init[x.term] = { on: true, tr: x.translation ?? "" };
      setRows(init);
    } finally {
      setLoading(false);
    }
  }

  // AI-translate the candidates that still have no translation.
  async function translateEmpty() {
    if (!cands) return;
    const todo = cands.filter((c) => !(rows[c.term]?.tr ?? "").trim());
    if (todo.length === 0) return;
    setTranslating(true);
    setMsg(null);
    try {
      const res = await api.translateTexts(todo.map((c) => c.term), activeConfig());
      setRows((r) => {
        const next = { ...r };
        todo.forEach((c, i) => {
          const t = res[i];
          if (t) next[c.term] = { ...next[c.term], tr: t };
        });
        return next;
      });
    } catch (e) {
      setMsg(String(e));
    } finally {
      setTranslating(false);
    }
  }

  async function addSelected() {
    if (!cands) return;
    const items: [string, string][] = cands
      .filter((c) => rows[c.term]?.on && rows[c.term]?.tr.trim())
      .map((c) => [c.term, rows[c.term].tr.trim()]);
    const n = await api.glossaryAddBulk(items);
    setCands(null);
    setMsg(`Added ${n}`);
    onAdded();
  }

  if (!cands) {
    return (
      <div className="suggest-bar">
        <button className="ghost" onClick={suggest} disabled={loading}>
          {loading ? "Scanning…" : "✨ Auto-suggest from game"}
        </button>
        {msg && <span className="ok-msg">{msg}</span>}
      </div>
    );
  }

  return (
    <div className="suggest-panel">
      <div className="suggest-head">
        <strong>{cands.length} candidates</strong>
        <span className="hint">
          character/enemy names + terms.
        </span>
        {msg && <span className="error">{msg}</span>}
        <button className="ghost" onClick={translateEmpty} disabled={translating}>
          {translating ? "Translating…" : "🌐 Translate empty (AI)"}
        </button>
        <button className="primary" onClick={addSelected}>
          Add selected
        </button>
        <button className="ghost" onClick={() => setCands(null)}>
          Cancel
        </button>
      </div>
      <div className="suggest-list">
        {cands.map((c) => (
          <div key={c.term} className="suggest-row">
            <input
              type="checkbox"
              checked={rows[c.term]?.on ?? true}
              onChange={(e) =>
                setRows((r) => ({ ...r, [c.term]: { ...r[c.term], on: e.target.checked } }))
              }
            />
            <span className="cand-term" title={`${c.kind} ×${c.count}`}>
              {c.term}
            </span>
            <input
              className="cand-tr"
              placeholder="translation…"
              value={rows[c.term]?.tr ?? ""}
              onChange={(e) =>
                setRows((r) => ({ ...r, [c.term]: { ...r[c.term], tr: e.target.value } }))
              }
            />
          </div>
        ))}
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
