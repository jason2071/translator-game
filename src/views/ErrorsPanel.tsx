import { useEffect, useState } from "react";
import { api, type TransUnit } from "../ipc";
import { useStore } from "../store";
import { useErrors } from "../errors";

export default function ErrorsPanel({ onClose }: { onClose: () => void }) {
  const [units, setUnits] = useState<TransUnit[] | null>(null);
  const setFilter = useStore((s) => s.setFilter);
  const byId = useErrors((s) => s.byId);

  useEffect(() => {
    api.listUnits({ status: "Failed", limit: 5000 }).then(setUnits);
  }, []);

  if (units === null) return <p className="hint">Loading…</p>;
  if (units.length === 0) return <p className="ok-msg">✓ No translation errors.</p>;

  return (
    <div className="errors">
      <p className="hint">
        {units.length} unit(s) failed. Click one to jump to its file, then use “Retry
        failed”.
      </p>
      <ul className="errors-list">
        {units.map((u) => (
          <li key={u.id}>
            <button
              className="err-row"
              onClick={() => {
                setFilter({ status: "Failed", file: u.file });
                onClose();
              }}
              title="Jump to this file’s failed lines"
            >
              <span className="err-src">{u.source}</span>
              <span className="err-meta">
                <span className="err-file">{u.file}</span>
                <span className="err-reason">{byId[u.id] ?? "Translation failed"}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
