import { useState } from "react";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings } from "../settings";
import { PROVIDER_LABELS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";

export default function TranslateBar({ openSettings }: { openSettings: () => void }) {
  const filter = useStore((s) => s.filter);
  const stats = useStore((s) => s.stats);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const activeConfig = useSettings((s) => s.activeConfig);

  // Only this Run's own status gates the controls; a glossary job runs in the
  // shared queue and does not lock the Run button (it just queues).
  const unitsPhase = useTranslation((s) => s.units.phase);
  const enqueue = useTranslation((s) => s.enqueue);
  const cancel = useTranslation((s) => s.cancel);
  const running = unitsPhase !== "idle"; // queued or running

  const [mode, setMode] = useState<"shown" | "all">("shown");
  const [overwrite, setOverwrite] = useState(false);
  const [summary, setSummary] = useState<TranslateSummary | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function translate(scope: TranslateScope) {
    setErr(null);
    setSummary(null);
    try {
      const res = await enqueue("units", () => api.translateUnits(scope, activeConfig()));
      setSummary(res);
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      setErr(String(e));
    }
  }

  function run() {
    translate(
      mode === "all"
        ? { filter: { untranslatedOnly: true }, overwrite }
        : { filter, overwrite }
    );
  }

  // Re-translate only the units that failed a previous run, no manual filtering.
  function retryFailed() {
    translate({ filter: { status: "Failed" } });
  }

  const failed = stats?.failed ?? 0;

  return (
    <div className="toolbar">
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

      <label
        className="chk"
        title={
          mode === "all"
            ? "No effect: 'All untranslated' never touches existing translations"
            : "Re-translate units that already have a translation"
        }
      >
        <input
          type="checkbox"
          checked={overwrite && mode !== "all"}
          onChange={(e) => setOverwrite(e.target.checked)}
          disabled={running || mode === "all"}
        />
        Overwrite existing
      </label>

      <button
        className="chip-btn"
        onClick={openSettings}
        disabled={running}
        style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem" }}
      >
        {PROVIDER_LABELS[active]} <Icon name="settings" size={13} />
      </button>

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
        <button
          className="ghost"
          onClick={retryFailed}
          title="Re-translate every unit that failed a previous run"
          style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem" }}
        >
          <Icon name="retry" size={14} /> Retry failed ({failed})
        </button>
      )}

      {/* Separate status per kind: the Run over units and the glossary job. */}
      <TransProgress kind="units" />
      <TransProgress kind="glossary" />

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
