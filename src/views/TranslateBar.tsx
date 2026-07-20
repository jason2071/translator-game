import { useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings, PROVIDER_LABELS_SHORT, PROVIDER_KINDS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";

export default function TranslateBar({ onOpenErrors }: { onOpenErrors: () => void }) {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const stats = useStore((s) => s.stats);
  // Count of units matching the current filter (== the "N shown" search matches).
  const total = useStore((s) => s.total);
  const refreshTotal = useStore((s) => s.refreshTotal);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const fillSourceForFilter = useStore((s) => s.fillSourceForFilter);
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
  const [rescanning, setRescanning] = useState(false);
  const [rescanMsg, setRescanMsg] = useState<string | null>(null);

  async function translate(scope: TranslateScope) {
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

  // Copy the source text into the translation for the current view's still-empty
  // (Untranslated/Failed) lines — a manual fallback when heavy inline markup makes
  // an AI pass unreliable (keep the codes, edit only the words). Existing
  // translations are never overwritten.
  async function fillSource() {
    const ok = await ask(
      `Fill the untranslated/failed line(s) in this view with their source text? ` +
        `You can then hand-edit them; existing translations are left untouched.`,
      { title: "Copy source → translation?", kind: "info" }
    );
    if (!ok) return;
    setErr(null);
    setSummary(null);
    try {
      await fillSourceForFilter();
      await refreshMeta();
      await refreshTotal();
    } catch (e) {
      setErr(String(e));
    }
  }

  // Re-scan the game into this project: pick up text the engine gained support
  // for since import (new tiers, new harvests) + backfill speaker context —
  // keeping every translation. Same op as the Glossary panel's "Rescan game".
  async function rescan() {
    setRescanning(true);
    setErr(null);
    setSummary(null);
    setRescanMsg(null);
    try {
      const r = await api.rescanProject();
      await refreshMeta();
      await refreshTotal();
      await useStore.getState().reloadUnits();
      setRescanMsg(
        r.added > 0 || r.contextFilled > 0
          ? `Rescanned: +${r.added} new line(s), filled ${r.contextFilled} speaker(s).`
          : "Rescanned — nothing new in the game.",
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setRescanning(false);
    }
  }

  const failed = stats?.failed ?? 0;

  // Secondary/contextual actions, shown as visible buttons next to Run (no
  // overflow menu — everything findable at a glance). Contextual ones still only
  // appear when they apply.
  const showRetranslate = (filter.search || filter.context) && total > 0 && !running;
  const showCopySource = (filter.search || filter.context || filter.file) && total > 0 && !running;
  // Long character names would blow the button row up — the full name stays in
  // the tooltip.
  const ctx = filter.context;
  const ctxShort = ctx && ctx.length > 14 ? `${ctx.slice(0, 13)}…` : ctx;

  return (
    <>
      <div className="toolbar">
        {/* Left: Run configuration — language pair, AI provider, target scope,
            and the overwrite option (everything that shapes what Run does). */}
        <div className="tb-config">
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

          <select
            className="tb-provider"
            value={active}
            onChange={(e) => setActive(e.target.value as typeof active)}
            disabled={running}
            title="AI provider used for Run (configure providers in Settings)"
          >
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {PROVIDER_LABELS_SHORT[k]}
              </option>
            ))}
          </select>

          <span
            className="tb-scope"
            title="Run translates this — click a file (or 'All files') in the sidebar to change it"
          >
            <b>{filter.file ?? "All files"}</b>
          </span>

          <label className="chk" title="Re-translate units that already have a translation">
            <input
              type="checkbox"
              checked={overwrite}
              onChange={(e) => setOverwrite(e.target.checked)}
              disabled={running}
            />
            Overwrite
          </label>
        </div>

        {/* Right: the secondary actions as plain visible buttons (contextual
            ones appear only when relevant), then the primary Run/Cancel at the
            far right edge. */}
        <div className="tb-actions">
          {showRetranslate && (
            <button
              className="ghost tb-act"
              onClick={retranslateMatches}
              title={
                ctx
                  ? `Re-translate every line of "${ctx}" (overwrites their translations)`
                  : "Re-translate every unit matching the current search (overwrites their translations)"
              }
            >
              <Icon name="retry" size={14} />
              {ctx ? `Re-translate ${ctxShort} (${total})` : `Re-translate (${total})`}
            </button>
          )}
          {showCopySource && (
            <button
              className="ghost tb-act"
              onClick={fillSource}
              title="Fill the untranslated/failed lines in this view with their source text, to hand-edit (keeps existing translations)"
            >
              <Icon name="copy" size={14} />
              Copy source
            </button>
          )}
          {failed > 0 && !running && (
            <button
              className="ghost tb-act"
              onClick={retryFailed}
              title="Re-translate every unit that failed a previous run"
            >
              <Icon name="retry" size={14} />
              Retry failed ({failed})
            </button>
          )}
          {failed > 0 && (
            <button
              className="ghost tb-act tb-act-warn"
              onClick={onOpenErrors}
              title="See which units failed and why"
            >
              <Icon name="warn" size={14} />
              Errors ({failed})
            </button>
          )}
          {!running && (
            <button
              className="ghost tb-act"
              onClick={() => {
                if (!rescanning) rescan();
              }}
              disabled={rescanning}
              title="Re-scan the game: pull in new text the engine now supports + fill in speakers on existing lines (keeps translations)"
            >
              <Icon name="retry" size={14} />
              {rescanning ? "Rescanning…" : "Rescan game"}
            </button>
          )}

          {(showRetranslate || showCopySource || failed > 0 || !running) && (
            <span className="tb-sep" />
          )}
          {!running ? (
            <button className="primary tb-run" onClick={run}>
              Run
            </button>
          ) : (
            <button className="ghost tb-run" onClick={() => cancel("units")}>
              Cancel
            </button>
          )}
        </div>
      </div>

      {(running || glossaryBusy || summary || err || rescanning || rescanMsg) && (
        <div className="tb-status">
          <TransProgress kind="units" />
          <TransProgress kind="glossary" />
          {rescanning && <span>Rescanning the game…</span>}
          {rescanMsg && !rescanning && <span className="export-ok">{rescanMsg}</span>}
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
