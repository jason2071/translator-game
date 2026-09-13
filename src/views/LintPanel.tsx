import { useEffect, useState } from "react";
import { api, type LintWarning } from "../ipc";
import { useStore } from "../store";

export default function LintPanel({ onClose }: { onClose: () => void }) {
  const [warnings, setWarnings] = useState<LintWarning[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const setFilter = useStore((s) => s.setFilter);

  async function run() {
    setWarnings(null);
    setError(null);
    try {
      setWarnings(await api.glossaryLint());
    } catch (e) {
      setWarnings([]);
      setError(String(e));
    }
  }
  useEffect(() => {
    run();
  }, []);

  if (warnings === null) return <p className="hint">Checking</p>;

  return (
    <div className="lint">
      <div className="lint-head">
        <span className="hint">Glossary wording</span>
        <button className="ghost" onClick={() => void run()}>Refresh</button>
      </div>
      {error && <p className="error">{error}</p>}
      {warnings.length === 0 ? (
        !error && <p className="ok-msg">✓ No glossary inconsistencies found.</p>
      ) : (
        <>
          <p className="hint">
            {warnings.length} translation(s) miss their glossary wording.
          </p>
          <ul className="lint-list">
            {warnings.map((w, i) => (
              <li key={i}>
                <button
                  className="link"
                  onClick={() => {
                    setFilter({ file: w.file });
                    onClose();
                  }}
                >
                  {w.file.split(/[\\/]/).pop() ?? w.file}
                </button>{" "}
                <strong>{w.term}</strong> → <em>{w.expected}</em>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
