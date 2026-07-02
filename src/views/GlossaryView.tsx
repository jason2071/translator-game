import { useEffect, useState } from "react";
import { api, type GlossaryEntry } from "../ipc";

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
