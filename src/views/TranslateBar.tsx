import { useState } from "react";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings } from "../settings";
import { PROVIDER_LABELS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";

export default function TranslateBar({ openSettings }: { openSettings: () => void }) {
  const filter = useStore((s) => s.filter);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const activeConfig = useSettings((s) => s.activeConfig);

  // Shared status: `running` is true whenever ANY translation (units or
  // glossary) is in flight, so the two never overlap and use one progress bar.
  const running = useTranslation((s) => s.busy);
  const runTranslation = useTranslation((s) => s.run);
  const cancel = useTranslation((s) => s.cancel);

  const [mode, setMode] = useState<"shown" | "all">("shown");
  const [overwrite, setOverwrite] = useState(false);
  const [summary, setSummary] = useState<TranslateSummary | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setErr(null);
    setSummary(null);
    const scope: TranslateScope =
      mode === "all"
        ? { filter: { untranslatedOnly: true }, overwrite }
        : { filter, overwrite };
    try {
      const res = await runTranslation("units", () => api.translateUnits(scope, activeConfig()));
      setSummary(res);
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <div className="translate-bar">
      <span className="tb-label">AI translate</span>

      <div className="lang-switch">
        <select
          value={project?.sourceLang ?? "Auto"}
          onChange={(e) => setLanguages(e.target.value, project?.targetLang ?? "Thai")}
          disabled={running}
          title="Source language"
        >
          {SOURCE_LANGS.map((l) => (
            <option key={l} value={l}>
              {l}
            </option>
          ))}
        </select>
        <span className="arrow">→</span>
        <select
          value={project?.targetLang ?? "Thai"}
          onChange={(e) => setLanguages(project?.sourceLang ?? "Auto", e.target.value)}
          disabled={running}
          title="Target language"
        >
          {TARGET_LANGS.map((l) => (
            <option key={l} value={l}>
              {l}
            </option>
          ))}
        </select>
      </div>

      <select value={mode} onChange={(e) => setMode(e.target.value as "shown" | "all")} disabled={running}>
        <option value="shown">Shown (current filter)</option>
        <option value="all">All untranslated</option>
      </select>

      <label className="chk">
        <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} disabled={running} />
        Overwrite existing
      </label>

      <button className="chip-btn" onClick={openSettings} disabled={running}>
        {PROVIDER_LABELS[active]} ⚙
      </button>

      {!running ? (
        <button className="primary" onClick={run}>
          Run
        </button>
      ) : (
        <button className="ghost" onClick={cancel}>
          Cancel
        </button>
      )}

      <TransProgress />

      {summary && (
        <span className="export-ok">
          {summary.cancelled ? "Cancelled — " : "Done — "}
          {summary.translated} translated
          {summary.reused > 0 ? `, ${summary.reused} reused` : ""}
          {summary.failed > 0 ? `, ${summary.failed} failed` : ""}
        </span>
      )}
      {err && <span className="error">{err}</span>}
    </div>
  );
}
