import { useEffect, useState } from "react";
import { api, type LintWarning } from "../ipc";
import { useStore } from "../store";

export default function LintPanel({ onClose }: { onClose: () => void }) {
  const [warnings, setWarnings] = useState<LintWarning[] | null>(null);
  const setFilter = useStore((s) => s.setFilter);

  async function run() {
    setWarnings(await api.glossaryLint());
  }
  useEffect(() => {
    run();
  }, []);

  if (warnings === null) return <p className="hint">Checking…</p>;

  return (
    <div className="lint">
      {warnings.length === 0 ? (
        <p className="ok-msg">✓ No glossary inconsistencies found.</p>
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
                  {w.file}
                </button>{" "}
                — term <strong>{w.term}</strong> should map to{" "}
                <em>{w.expected}</em>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
