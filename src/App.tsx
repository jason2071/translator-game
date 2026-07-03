import { useState } from "react";
import { useStore } from "./store";
import { useTheme } from "./theme";
import { api, type ExportResult, type Status } from "./ipc";
import ImportView from "./views/ImportView";
import GridView from "./views/GridView";
import GlossaryView from "./views/GlossaryView";
import LintPanel from "./views/LintPanel";
import SettingsView from "./views/SettingsView";
import TranslateBar from "./views/TranslateBar";
import { Modal } from "./components/Modal";

type Panel = "none" | "glossary" | "lint" | "settings";

export default function App() {
  const project = useStore((s) => s.project);
  const [panel, setPanel] = useState<Panel>("none");
  if (!project) return <ImportView />;
  return (
    <div className="app">
      <TopBar openPanel={setPanel} />
      <TranslateBar openSettings={() => setPanel("settings")} />
      <GridView />
      {panel === "glossary" && (
        <Modal title="Glossary" onClose={() => setPanel("none")}>
          <GlossaryView />
        </Modal>
      )}
      {panel === "lint" && (
        <Modal title="Glossary lint" onClose={() => setPanel("none")}>
          <LintPanel onClose={() => setPanel("none")} />
        </Modal>
      )}
      {panel === "settings" && (
        <Modal title="AI providers & settings" onClose={() => setPanel("none")}>
          <SettingsView />
        </Modal>
      )}
    </div>
  );
}

function TopBar({ openPanel }: { openPanel: (p: Panel) => void }) {
  const project = useStore((s) => s.project)!;
  const stats = useStore((s) => s.stats);
  const closeProject = useStore((s) => s.closeProject);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const setFilter = useStore((s) => s.setFilter);
  // Clicking a stat chip filters the grid to that status (total clears it).
  const goStatus = (status?: Status) => setFilter({ status, untranslatedOnly: false });
  const [exporting, setExporting] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [tmMsg, setTmMsg] = useState<string | null>(null);
  const [applyingTm, setApplyingTm] = useState(false);

  async function doApplyTm() {
    setTmMsg(null);
    setErr(null);
    setApplyingTm(true);
    try {
      const n = await api.applyTm();
      setTmMsg(`Filled ${n} from memory`);
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      setErr(String(e));
    } finally {
      setApplyingTm(false);
    }
  }

  async function doExport() {
    setExporting(true);
    setErr(null);
    setResult(null);
    try {
      const r = await api.exportProject(true);
      setResult(r);
      await refreshMeta();
    } catch (e) {
      setErr(String(e));
    } finally {
      setExporting(false);
    }
  }

  return (
    <header className="topbar">
      <div className="tb-left">
        <strong>{project.engineName}</strong>
        <span className="tb-path">{project.root}</span>
      </div>

      <div className="tb-stats">
        {stats && (
          <>
            <Chip label="total" value={stats.total} onClick={() => goStatus(undefined)} />
            <Chip label="todo" value={stats.untranslated} tone="muted" onClick={() => goStatus("Untranslated")} />
            {stats.failed > 0 && (
              <Chip label="failed" value={stats.failed} tone="err" onClick={() => goStatus("Failed")} />
            )}
            <Chip label="draft" value={stats.draft} tone="warn" onClick={() => goStatus("Draft")} />
            <Chip label="done" value={stats.translated + stats.reviewed} tone="ok" onClick={() => goStatus("Translated")} />
            {stats.locked > 0 && (
              <Chip label="locked" value={stats.locked} tone="muted" onClick={() => goStatus("Locked")} />
            )}
          </>
        )}
      </div>

      <div className="tb-actions">
        {result && (
          <span
            className="export-ok"
            title={`Exported ${result.unitsApplied} units → ${result.filesWritten} files${
              result.backupDir ? " (backup saved)" : ""
            }`}
          >
            Exported {result.unitsApplied} units → {result.filesWritten} files
            {result.backupDir ? " (backup saved)" : ""}
          </span>
        )}
        {tmMsg && (
          <span className="export-ok" title={tmMsg}>
            {tmMsg}
          </span>
        )}
        {err && (
          <span className="error" title={err}>
            {err}
          </span>
        )}
        <button
          className="ghost"
          onClick={doApplyTm}
          disabled={applyingTm}
          title="Fill from translation memory + duplicates"
        >
          {applyingTm ? "Applying…" : "Apply TM"}
        </button>
        <button className="ghost" onClick={() => openPanel("glossary")}>
          Glossary
        </button>
        <button className="ghost" onClick={() => openPanel("lint")}>
          Lint
        </button>
        <button className="primary" onClick={doExport} disabled={exporting}>
          {exporting ? "Exporting…" : "Export → game"}
        </button>
        <ThemeToggle />
        <button className="ghost" onClick={closeProject}>
          Close
        </button>
      </div>
    </header>
  );
}

function ThemeToggle() {
  const theme = useTheme((s) => s.theme);
  const toggle = useTheme((s) => s.toggle);
  return (
    <button
      className="ghost"
      onClick={toggle}
      title="Toggle light/dark theme"
      aria-label="Toggle light/dark theme"
    >
      {theme === "dark" ? "☀" : "🌙"}
    </button>
  );
}

function Chip({
  label,
  value,
  tone,
  onClick,
}: {
  label: string;
  value: number;
  tone?: "muted" | "warn" | "ok" | "err";
  onClick?: () => void;
}) {
  const clickable = !!onClick;
  return (
    <span
      className={`chip ${tone ?? ""}${clickable ? " clickable" : ""}`}
      onClick={onClick}
      role={clickable ? "button" : undefined}
      tabIndex={clickable ? 0 : undefined}
      title={clickable ? `Filter: ${label}` : undefined}
      onKeyDown={
        clickable
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick!();
              }
            }
          : undefined
      }
    >
      <span className="chip-val">{value}</span>
      <span className="chip-lbl">{label}</span>
    </span>
  );
}
