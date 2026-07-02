import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type Progress, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings } from "../settings";
import { PROVIDER_LABELS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";

export default function TranslateBar({ openSettings }: { openSettings: () => void }) {
  const filter = useStore((s) => s.filter);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const activeConfig = useSettings((s) => s.activeConfig);

  const [mode, setMode] = useState<"shown" | "all">("shown");
  const [overwrite, setOverwrite] = useState(false);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [summary, setSummary] = useState<TranslateSummary | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);

  useEffect(() => () => void unlistenRef.current?.(), []);

  async function run() {
    setRunning(true);
    setErr(null);
    setSummary(null);
    setProgress(null);
    unlistenRef.current = await api.onProgress(setProgress);
    const scope: TranslateScope =
      mode === "all"
        ? { filter: { untranslatedOnly: true }, overwrite }
        : { filter, overwrite };
    try {
      const res = await api.translateUnits(scope, activeConfig());
      setSummary(res);
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      setErr(String(e));
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setRunning(false);
    }
  }

  const pct = progress && progress.total > 0 ? Math.round((progress.done / progress.total) * 100) : 0;

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
        <button className="ghost" onClick={() => api.cancelTranslation()}>
          Cancel
        </button>
      )}

      {(running || progress) && (
        <div className="tb-progress">
          <div className="bar">
            <div className="bar-fill" style={{ width: `${pct}%` }} />
          </div>
          <span className="tb-count">
            {progress ? `${progress.done}/${progress.total}` : "…"}
            {progress && progress.failed > 0 ? ` · ${progress.failed} failed` : ""}
          </span>
        </div>
      )}

      {summary && (
        <span className="export-ok">
          {summary.cancelled ? "Cancelled — " : "Done — "}
          {summary.translated} translated
          {summary.failed > 0 ? `, ${summary.failed} failed` : ""}
        </span>
      )}
      {err && <span className="error">{err}</span>}
    </div>
  );
}
