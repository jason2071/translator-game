import { useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings, PROVIDER_LABELS, PROVIDER_KINDS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";

export default function TranslateBar({ onOpenErrors }: { onOpenErrors: () => void }) {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const stats = useStore((s) => s.stats);
  const refreshTotal = useStore((s) => s.refreshTotal);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const setActive = useSettings((s) => s.setActive);
  const activeConfig = useSettings((s) => s.activeConfig);

  // Only this Run's own status gates the controls; a glossary job runs in the
  // shared queue and does not lock the Run button (it just queues).
  const unitsPhase = useTranslation((s) => s.units.phase);
  const glossaryBusy = useTranslation((s) => s.glossary.phase !== "idle");
  const enqueue = useTranslation((s) => s.enqueue);
  const cancel = useTranslation((s) => s.cancel);
  const running = unitsPhase !== "idle"; // queued or running

  const [overwrite, setOverwrite] = useState(false);
  const [summary, setSummary] = useState<TranslateSummary | null>(null);
  const [err, setErr] = useState<string | null>(null);

  // Surface the first transport-level error (AI unreachable / rate-limited) the
  // moment it happens, so a Run doesn't silently mark everything Failed.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    api
      .onTranslateError((msg) => setErr(`AI error: ${msg}`))
      .then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  async function translate(scope: TranslateScope) {
    setErr(null);
    setSummary(null);
    try {
      const res = await enqueue("units", () => api.translateUnits(scope, activeConfig()));
      setSummary(res);
      // A transport error may have occurred even though the command resolved
      // (the Run keeps going and marks the rest Failed).
      if (res.error) setErr(`AI error: ${res.error}`);
      // The visible rows were live-patched during the Run; just refresh the
      // sidebar counts and the total (no full reload → no scroll jump).
      await refreshMeta();
      await refreshTotal();
    } catch (e) {
      setErr(String(e));
    }
  }

  // Translate the file selected in the sidebar (its untranslated + Failed units),
  // or the whole project when "All files" is selected (filter.file === undefined).
  function run() {
    translate({ filter: { file: filter.file }, overwrite });
  }

  // Re-translate only the units that failed a previous run, no manual filtering.
  function retryFailed() {
    translate({ filter: { status: "Failed" } });
  }

  const failed = stats?.failed ?? 0;

  return (
    <>
      <div className="toolbar">
        <div className="tb-config">
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

          <span
            className="tb-scope"
            title="Run translates this — click a file (or 'All files') in the sidebar to change it"
          >
            Target: <b>{filter.file ?? "All files"}</b>
          </span>

          <label className="chk" title="Re-translate units that already have a translation">
            <input
              type="checkbox"
              checked={overwrite}
              onChange={(e) => setOverwrite(e.target.checked)}
              disabled={running}
            />
            Overwrite existing
          </label>

          <select
            value={active}
            onChange={(e) => setActive(e.target.value as typeof active)}
            disabled={running}
            title="AI provider used for Run (configure providers in Settings)"
          >
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {PROVIDER_LABELS[k]}
              </option>
            ))}
          </select>
        </div>

        <div className="tb-actions">
          {!running ? (
            <button className="primary" onClick={run}>
              Run
            </button>
          ) : (
            <button className="ghost" onClick={() => cancel("units")}>
              Cancel
            </button>
          )}

          {failed > 0 && !running && (
            <button className="ghost tb-icon-btn" onClick={retryFailed} title="Re-translate every unit that failed a previous run">
              <Icon name="retry" size={14} /> Retry failed ({failed})
            </button>
          )}

          <button
            className="ghost tb-icon-btn"
            onClick={onOpenErrors}
            disabled={failed === 0}
            title="See which units failed and why"
          >
            <Icon name="warn" size={14} /> Errors ({failed})
          </button>
        </div>
      </div>

      {(running || glossaryBusy || summary || err) && (
        <div className="tb-status">
          <TransProgress kind="units" />
          <TransProgress kind="glossary" />
          {summary && (
            <span className="export-ok">
              {summary.cancelled ? "Cancelled — " : "Done — "}
              {summary.translated} translated
              {summary.reused > 0 ? `, ${summary.reused} reused` : ""}
              {summary.failed > 0 && (
                <>
                  {", "}
                  <button
                    className="linklike failed-link"
                    onClick={() => setFilter({ status: "Failed", untranslatedOnly: false })}
                    title="Show the units that failed so you can retry or fix them"
                  >
                    {summary.failed} failed
                  </button>
                </>
              )}
            </span>
          )}
          {err && <span className="error">{err}</span>}
        </div>
      )}
    </>
  );
}
