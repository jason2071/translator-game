import { useState } from "react";
import { useStore } from "./store";
import { useTheme } from "./theme";
import { api, type ExportResult } from "./ipc";
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
            <Chip label="total" value={stats.total} />
            <Chip label="todo" value={stats.untranslated} tone="muted" />
            {stats.failed > 0 && <Chip label="failed" value={stats.failed} tone="err" />}
            <Chip label="draft" value={stats.draft} tone="warn" />
            <Chip label="done" value={stats.translated + stats.reviewed} tone="ok" />
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
    <button className="ghost" onClick={toggle} title="Toggle light/dark theme">
      {theme === "dark" ? "☀" : "🌙"}
    </button>
  );
}

function Chip({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "muted" | "warn" | "ok" | "err";
}) {
  return (
    <span className={`chip ${tone ?? ""}`}>
      <span className="chip-val">{value}</span>
      <span className="chip-lbl">{label}</span>
    </span>
  );
}
