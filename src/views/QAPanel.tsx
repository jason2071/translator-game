import { useState } from "react";
import ErrorsPanel from "./ErrorsPanel";
import LintPanel from "./LintPanel";

type QATab = "errors" | "lint";

export default function QAPanel({
  initialTab,
  onClose,
}: {
  initialTab: QATab;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<QATab>(initialTab);

  return (
    <div className="qa-panel">
      <div className="qa-tabs" role="tablist" aria-label="Quality checks">
        <button className={tab === "errors" ? "active" : ""} role="tab" aria-selected={tab === "errors"} onClick={() => setTab("errors")}>Errors</button>
        <button className={tab === "lint" ? "active" : ""} role="tab" aria-selected={tab === "lint"} onClick={() => setTab("lint")}>Glossary</button>
      </div>
      {tab === "errors" ? <ErrorsPanel onClose={onClose} /> : <LintPanel onClose={onClose} />}
    </div>
  );
}
