import { useEffect, useState } from "react";
import { api, type TransUnit } from "../ipc";
import { useStore } from "../store";
import { useErrors } from "../errors";

export default function ErrorsPanel({ onClose }: { onClose: () => void }) {
  const [units, setUnits] = useState<TransUnit[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const setFilter = useStore((s) => s.setFilter);
  const failedTotal = useStore((s) => s.stats?.failed ?? 0);
  const byId = useErrors((s) => s.byId);

  useEffect(() => {
    api.listUnits({ status: "Failed", limit: 5000 })
      .then(setUnits)
      .catch((e) => {
        setError(String(e));
        setUnits([]);
      });
  }, []);

  if (units === null) return <p className="hint">Loading</p>;
  if (error) return <p className="error">{error}</p>;
  if (units.length === 0) return <p className="ok-msg">✓ No translation errors.</p>;

  const total = Math.max(failedTotal, units.length);

  return (
    <div className="errors">
      <p className="hint">
        {total.toLocaleString()} failed. Open a file to review and retry it.
      </p>
      {total > units.length && (
        <p className="hint">Showing the first {units.length.toLocaleString()} items.</p>
      )}
      <ul className="errors-list">
        {units.map((u) => (
          <li key={u.id}>
            <button
              className="err-row"
              onClick={() => {
                setFilter({ status: "Failed", file: u.file });
                onClose();
              }}
              title="Open this file"
            >
              <span className="err-src">{u.source}</span>
              <span className="err-meta">
                <span className="err-file">{u.file.split(/[\\/]/).pop() ?? u.file}</span>
                <span className="err-reason">{byId[u.id] ?? "Translation failed"}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
