import { useState, type CSSProperties } from "react";
import { api, type ExportResult, type Status } from "../ipc";
import { useStore } from "../store";
import { useTheme } from "../theme";
import { statusColor } from "../status";
import { Icon } from "./Icon";

type Panel = "none" | "glossary" | "lint" | "settings";

export function Sidebar({
  openPanel,
  collapsed,
  onToggleCollapse,
}: {
  openPanel: (p: Panel) => void;
  collapsed: boolean;
  onToggleCollapse: () => void;
}) {
  const project = useStore((s) => s.project)!;
  const stats = useStore((s) => s.stats);
  const files = useStore((s) => s.files);
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const closeProject = useStore((s) => s.closeProject);
  const reextract = useStore((s) => s.reextract);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const theme = useTheme((s) => s.theme);
  const toggleTheme = useTheme((s) => s.toggle);

  const [exporting, setExporting] = useState(false);
  const [reimporting, setReimporting] = useState(false);
  const [applyingTm, setApplyingTm] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function doApplyTm() {
    setMsg(null);
    setErr(null);
    setApplyingTm(true);
    try {
      const n = await api.applyTm();
      setMsg(`Filled ${n} from memory`);
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      setErr(String(e));
    } finally {
      setApplyingTm(false);
    }
  }

  async function doReextract() {
    if (
      !window.confirm(
        "Re-read the game and rebuild the string list? Translations already done are kept (matched by source); glossary and memory are unaffected."
      )
    )
      return;
    setMsg(null);
    setErr(null);
    setReimporting(true);
    try {
      await reextract();
      setMsg("Re-imported from the game");
    } catch (e) {
      setErr(String(e));
    } finally {
      setReimporting(false);
    }
  }

  async function doExport() {
    setExporting(true);
    setErr(null);
    setResult(null);
    try {
      const r = await api.exportProject(true);
      setResult(r);
      setMsg(`Exported ${r.unitsApplied} units → ${r.filesWritten} files`);
      await refreshMeta();
    } catch (e) {
      setErr(String(e));
    } finally {
      setExporting(false);
    }
  }

  const pct =
    stats && stats.total > 0
      ? Math.round(((stats.total - stats.untranslated) / stats.total) * 100)
      : 0;

  const statusRows: { status?: Status; label: string; count: number; color: string }[] = stats
    ? [
        { status: undefined, label: "All", count: stats.total, color: "var(--subtle)" },
        { status: "Untranslated", label: "Todo", count: stats.untranslated, color: statusColor("Untranslated") },
        { status: "Failed", label: "Failed", count: stats.failed, color: statusColor("Failed") },
        { status: "Draft", label: "Draft", count: stats.draft, color: statusColor("Draft") },
        { status: "Translated", label: "Translated", count: stats.translated, color: statusColor("Translated") },
        { status: "Reviewed", label: "Reviewed", count: stats.reviewed, color: statusColor("Reviewed") },
        { status: "Locked", label: "Locked", count: stats.locked, color: statusColor("Locked") },
      ]
    : [];

  const allCount = files.reduce((a, f) => a + f.count, 0);

  return (
    <aside className="sidebar">
      <div className="sb-top">
        <div className="sb-title">
          <span className="sb-name">{project.engineName}</span>
          <span className="sb-path" title={project.root}>
            {project.root}
          </span>
        </div>
        <button
          className="iconbtn"
          onClick={onToggleCollapse}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          title={collapsed ? "Expand" : "Collapse"}
        >
          <Icon name={collapsed ? "chevron-right" : "chevron-left"} />
        </button>
      </div>

      <div className="sb-progress">
        <div className="ring" style={{ "--pct": pct } as CSSProperties}>
          <span className="ring-pct">{pct}%</span>
        </div>
        <div className="sb-sub">
          <span>
            <b>{stats?.untranslated ?? 0}</b> todo
          </span>
          {(stats?.failed ?? 0) > 0 && (
            <span>
              <b>{stats!.failed}</b> failed
            </span>
          )}
          <span>
            <b>{stats ? stats.translated + stats.reviewed : 0}</b> done
          </span>
        </div>
      </div>

      <div className="sb-scroll">
        <div className="sb-section-title">Status</div>
        <div className="sb-list">
          {statusRows.map((r) => (
            <button
              key={r.label}
              className={`sb-item${(filter.status ?? undefined) === r.status ? " active" : ""}`}
              onClick={() => setFilter({ status: r.status, untranslatedOnly: false })}
            >
              <span className="st-dot" style={{ background: r.color }} />
              <span className="lbl">{r.label}</span>
              <span className="count">{r.count}</span>
            </button>
          ))}
        </div>

        <div className="sb-section-title">Files</div>
        <div className="sb-list">
          <button
            className={`sb-item${!filter.file ? " active" : ""}`}
            onClick={() => setFilter({ file: undefined })}
          >
            <span className="lbl">All files</span>
            <span className="count">{allCount}</span>
          </button>
          {files.map((f) => (
            <button
              key={f.file}
              className={`sb-item${filter.file === f.file ? " active" : ""}`}
              onClick={() => setFilter({ file: f.file })}
              title={f.file}
            >
              <span className="lbl">{f.file}</span>
              <span className="count">{f.count}</span>
            </button>
          ))}
        </div>
      </div>

      {(msg || err) && (
        <div className={`sb-msg ${err ? "error" : "ok-msg"}`} title={err ?? msg ?? ""}>
          {err ?? msg}
          {result?.backupDir ? " (backup saved)" : ""}
        </div>
      )}

      <div className="sb-actions">
        <button className="ghost" onClick={doApplyTm} disabled={applyingTm} title="Fill from translation memory + duplicates">
          <Icon name="memory" />
          <span className="lbl">{applyingTm ? "Applying…" : "Apply TM"}</span>
        </button>
        <button className="ghost" onClick={doReextract} disabled={reimporting} title="Re-read the game with the current extractor (keeps done translations)">
          <Icon name="retry" />
          <span className="lbl">{reimporting ? "Re-importing…" : "Re-import"}</span>
        </button>
        <button className="ghost" onClick={() => openPanel("glossary")}>
          <Icon name="glossary" />
          <span className="lbl">Glossary</span>
        </button>
        <button className="ghost" onClick={() => openPanel("lint")}>
          <Icon name="lint" />
          <span className="lbl">Lint</span>
        </button>
        <button className="ghost" onClick={() => openPanel("settings")}>
          <Icon name="settings" />
          <span className="lbl">Settings</span>
        </button>
        <button className="primary full" onClick={doExport} disabled={exporting}>
          <Icon name="export" />
          <span className="lbl">{exporting ? "Exporting…" : "Export → game"}</span>
        </button>
        <div className="row">
          <button className="iconbtn" onClick={toggleTheme} aria-label="Toggle light/dark theme" title="Toggle theme">
            <Icon name={theme === "dark" ? "sun" : "moon"} />
          </button>
          <button className="iconbtn" onClick={closeProject} aria-label="Close project" title="Close project">
            <Icon name="close" />
          </button>
        </div>
      </div>
    </aside>
  );
}
