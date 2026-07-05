import { useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type TranslateScope, type TranslateSummary } from "../ipc";
import { useStore } from "../store";
import { useSettings, PROVIDER_LABELS, PROVIDER_KINDS } from "../settings";
import { SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { useTranslation } from "../translation";
import TransProgress from "../components/TransProgress";
import { Icon } from "../components/Icon";

export default function TranslateBar() {
  const filter = useStore((s) => s.filter);
  const stats = useStore((s) => s.stats);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const project = useStore((s) => s.project);
  const setLanguages = useStore((s) => s.setLanguages);
  const active = useSettings((s) => s.active);
  const setActive = useSettings((s) => s.setActive);
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
