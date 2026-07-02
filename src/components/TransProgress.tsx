import { useTranslation, type RunKind } from "../translation";

// Renders the shared translation progress. `only` scopes it to one kind so the
// TranslateBar shows unit runs and the glossary panel shows glossary runs, but
// both read the exact same store — identical numbers, one source of truth.
export default function TransProgress({ only }: { only?: RunKind }) {
  const busy = useTranslation((s) => s.busy);
  const kind = useTranslation((s) => s.kind);
  const progress = useTranslation((s) => s.progress);

  if (!busy || (only && kind !== only)) return null;
  const pct =
    progress && progress.total > 0
      ? Math.round((progress.done / progress.total) * 100)
      : 0;

  return (
    <div className="tb-progress">
      <div className="bar">
        <div className="bar-fill" style={{ width: `${pct}%` }} />
      </div>
      <span className="tb-count">
        {kind === "glossary" ? "glossary " : ""}
        {progress ? `${progress.done}/${progress.total}` : "…"}
        {progress && progress.failed > 0 ? ` · ${progress.failed} failed` : ""}
      </span>
    </div>
  );
}
