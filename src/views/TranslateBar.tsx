import { useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings, PROVIDER_LABELS_SHORT, PROVIDER_KINDS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";
import type { AppNotice } from "../notice";

export default function TranslateBar({
  onOpenErrors,
  notice,
  clearNotice,
}: {
  onOpenErrors: () => void;
  notice: AppNotice | null;
  clearNotice: () => void;
}) {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const stats = useStore((s) => s.stats);
  // Count of units matching the current filter (== the "N shown" search matches).
  const total = useStore((s) => s.total);
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
  // Only command-level failures (no API key / no project) surface here; per-unit
  // AI failures live in the Errors modal, so a Run no longer paints a red banner.
  const [err, setErr] = useState<string | null>(null);

  async function translate(scope: TranslateScope) {
    clearNotice();
    setErr(null);
    setSummary(null);
    try {
      const res = await enqueue("units", () => api.translateUnits(scope, activeConfig()));
      setSummary(res);
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

  // Re-translate every unit matching the current view (overwrites them). Unlike
  // Run (which scopes to the selected file), this sends the whole active filter —
  // search, status, character (context), untranslatedOnly — so it covers exactly
  // the "N shown" matches. A selected character re-translates just that actor.
  async function retranslateMatches() {
    const ok = await ask(
      filter.context
        ? `Re-translate all ${total} line(s) of "${filter.context}"? ` +
            `This overwrites their current translations.`
        : `Re-translate all ${total} unit(s) matching this search? ` +
            `This overwrites their current translations.`,
      {
        title: filter.context ? "Re-translate this character?" : "Re-translate search matches?",
        kind: "warning",
      }
    );
    if (!ok) return;
    // The store's filter holds only search/file/status/untranslatedOnly (never
    // limit/offset — the grid sets those per fetch), so it's the scope as-is; the
    // backend pages it and overrides limit/offset itself.
    translate({ filter, overwrite: true });
  }

  const failed = stats?.failed ?? 0;
  const scopeLabel = filter.file?.split(/[\\/]/).pop() ?? "All files";

  // Secondary/contextual actions, shown as visible buttons next to Run (no
  // overflow menu — everything findable at a glance). Contextual ones still only
  // appear when they apply.
  const showRetranslate = (filter.search || filter.context) && total > 0 && !running;
  return (
    <>
      <div className="toolbar">
        <span
          className="tb-scope"
          title={filter.file ?? "All files"}
        >
          <b>{scopeLabel}</b>
        </span>
        <details className="tb-options">
          <summary>Options</summary>
          <div className="tb-options-menu">
            <div className="tb-options-row">
              <span>Language</span>
              <div className="lang-switch">
                <select
                  value={project?.sourceLang ?? "Auto"}
                  onChange={(e) => setLanguages(e.target.value, project?.targetLang ?? "Thai")}
                  disabled={running}
                  aria-label="Source language"
                >
                  {SOURCE_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
                </select>
                <span className="arrow">→</span>
                <select
                  value={project?.targetLang ?? "Thai"}
                  onChange={(e) => setLanguages(project?.sourceLang ?? "Auto", e.target.value)}
                  disabled={running}
                  aria-label="Target language"
                >
                  {TARGET_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
                </select>
              </div>
            </div>
            <label className="tb-options-row">
              <span>Provider</span>
              <select
                className="tb-provider"
                value={active}
                onChange={(e) => setActive(e.target.value as typeof active)}
                disabled={running}
              >
                {PROVIDER_KINDS.map((k) => <option key={k} value={k}>{PROVIDER_LABELS_SHORT[k]}</option>)}
              </select>
            </label>
            <label className="chk" title="Re-translate lines that already have a translation">
              <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} disabled={running} />
              Overwrite
            </label>
            <div className="tb-options-actions">
              {showRetranslate && <button className="ghost tb-act" onClick={retranslateMatches}><Icon name="retry" size={14} />Retry</button>}
              {failed > 0 && !running && <button className="ghost tb-act" onClick={retryFailed}><Icon name="retry" size={14} />Retry</button>}
              {failed > 0 && <button className="ghost tb-act tb-act-warn" onClick={onOpenErrors}><Icon name="warn" size={14} />Errors</button>}
            </div>
          </div>
        </details>
        <div className="tb-actions">
          {!running ? (
            <button className="primary tb-run" onClick={run}>
              Translate
            </button>
          ) : (
            <button className="ghost tb-run" onClick={() => cancel("units")}>
              Cancel
            </button>
          )}
        </div>
      </div>

      <div className="tb-status" role="status" aria-live="polite">
        {running || glossaryBusy ? (
          <>
          <TransProgress kind="units" />
          <TransProgress kind="glossary" />
          </>
        ) : notice ? (
          <span className={`footer-notice ${notice.tone}`} title={notice.text}>{notice.text}</span>
        ) : err ? (
          <span className="error" title={err}>{err}</span>
        ) : summary ? (
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
        ) : (
          <span className="footer-default">Changes are saved automatically.</span>
        )}
      </div>
    </>
  );
}
