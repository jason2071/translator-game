import { useState } from "react";
import { useStore } from "../store";
import { useSettings, PROVIDER_LABELS_SHORT, PROVIDER_KINDS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import { useRun } from "../run";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";
import type { AppNotice } from "../notice";

export default function TranslateBar({
  onOpenErrors,
  notice,
}: {
  onOpenErrors: () => void;
  notice: AppNotice | null;
}) {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const stats = useStore((s) => s.stats);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const setActive = useSettings((s) => s.setActive);

  // Only this Run's own status gates the controls; a glossary job runs in the
  // shared queue and does not lock the Run button (it just queues).
  const glossaryBusy = useTranslation((s) => s.glossary.phase !== "idle");
  const cancel = useTranslation((s) => s.cancel);
  const running = useTranslation((s) => s.units.phase !== "idle"); // queued or running

  // Runs are started from several places (Run / Retry failed / the filter
  // bar's Re-translate); the shared outcome lands here and renders below.
  const run = useRun((s) => s.run);
  const summary = useRun((s) => s.summary);
  const err = useRun((s) => s.err);

  const [overwrite, setOverwrite] = useState(false);

  // Translate the file selected in the sidebar (its untranslated + Failed units),
  // or the whole project when "All files" is selected (filter.file === undefined).
  function runUnits() {
    void run({ filter: { file: filter.file }, overwrite });
  }

  // Re-translate only the units that failed a previous run, no manual filtering.
  function retryFailed() {
    void run({ filter: { status: "Failed" } });
  }

  const failed = stats?.failed ?? 0;
  const scopeLabel = filter.file?.split(/[\\/]/).pop() ?? "All files";

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
            <div className="tb-language-grid">
              <label className="tb-options-field">
                <span>From</span>
                <select
                  value={project?.sourceLang ?? "Auto"}
                  onChange={(e) => setLanguages(e.target.value, project?.targetLang ?? "Thai")}
                  disabled={running}
                  aria-label="Source language"
                >
                  {SOURCE_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
                </select>
              </label>
              <span className="tb-language-arrow" aria-hidden="true">→</span>
              <label className="tb-options-field">
                <span>To</span>
                <select
                  value={project?.targetLang ?? "Thai"}
                  onChange={(e) => setLanguages(project?.sourceLang ?? "Auto", e.target.value)}
                  disabled={running}
                  aria-label="Target language"
                >
                  {TARGET_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
                </select>
              </label>
            </div>
            <label className="tb-options-field">
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
            <label className="chk tb-overwrite" title="Re-translate lines that already have a translation">
              <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} disabled={running} />
              <span className="tb-overwrite-copy">
                <strong>Overwrite</strong>
                <small>Translate completed lines again</small>
              </span>
            </label>
          </div>
        </details>
        <div className="tb-actions">
          {/* Failure state lives on the toolbar, not in Options: it must be
              visible whenever anything failed. The chip opens the error list;
              the button next to it re-runs just those units. */}
          {failed > 0 && (
            <button
              className="ghost tb-act tb-act-warn"
              onClick={onOpenErrors}
              title="Open the failed units with the reason each one failed"
            >
              <Icon name="warn" size={14} />
              {failed.toLocaleString()} failed
            </button>
          )}
          {failed > 0 && !running && (
            <button
              className="ghost tb-act"
              onClick={retryFailed}
              title={`Re-run translation for the ${failed.toLocaleString()} unit(s) that failed`}
            >
              <Icon name="retry" size={14} />Retry failed
            </button>
          )}
          {!running ? (
            <button className="primary tb-run" onClick={runUnits}>
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
