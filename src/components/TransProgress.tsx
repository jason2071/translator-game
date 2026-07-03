import { useTranslation, type RunKind } from "../translation";

// Renders one kind's translation status (queued or running) from the shared
// store. Pass kind="units" and kind="glossary" to show them separately.
export default function TransProgress({ kind }: { kind: RunKind }) {
  const st = useTranslation((s) => (kind === "units" ? s.units : s.glossary));
  if (st.phase === "idle") return null;

  const label = kind === "glossary" ? "Glossary" : "Run";
  if (st.phase === "queued") {
    return (
      <div className="tb-progress">
        <span className="tb-count queued">{label}: queued…</span>
      </div>
    );
  }

  const p = st.progress;
  const pct = p && p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;
  return (
    <div className="tb-progress">
      <div className="bar">
        <div className="bar-fill" style={{ width: `${pct}%` }} />
      </div>
      <span className="tb-count">
        {label}: {p ? `${p.done}/${p.total}` : "…"}
        {p && p.failed > 0 ? ` · ${p.failed} failed` : ""}
      </span>
    </div>
  );
}
