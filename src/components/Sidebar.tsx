import { useState, useEffect } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { ask } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type Status } from "../ipc";
import { useStore } from "../store";
import { useTheme } from "../theme";
import { statusColor } from "../status";
import { useTranslation } from "../translation";
import { Icon } from "./Icon";
import type { AppNotice } from "../notice";

type Panel = "none" | "glossary" | "lint" | "settings";

export function Sidebar({
  openPanel,
  collapsed,
  onToggleCollapse,
  onNotice,
}: {
  openPanel: (p: Panel) => void;
  collapsed: boolean;
  onToggleCollapse: () => void;
  onNotice: (notice: AppNotice) => void;
}) {
  const project = useStore((s) => s.project)!;
  const stats = useStore((s) => s.stats);
  const files = useStore((s) => s.files);
  const characters = useStore((s) => s.characters);
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const closeProject = useStore((s) => s.closeProject);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const theme = useTheme((s) => s.theme);
  const toggleTheme = useTheme((s) => s.toggle);
  const unitsBusy = useTranslation((s) => s.units.phase !== "idle");

  const [exporting, setExporting] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [rescanning, setRescanning] = useState(false);
  const [applyingTm, setApplyingTm] = useState(false);
  const [version, setVersion] = useState("");

  // Stock game fonts often have no Thai glyphs, so offer to embed a Thai-capable font
  // on export. It stays opt-in so export never changes font files unexpectedly. Shown for the engines whose
  // `embed_font` does something (Ren'Py embeds its own font inside its tl/ path).
  const FONT_ENGINES = [
    "rpgmaker-mvmz",
    "rpgmaker-hendrix",
  ];
  const fontCapable = FONT_ENGINES.includes(project.engineId);
  const renpyThai = project.engineId === "renpy" && /^thai$/i.test(project.targetLang.trim());
  const [embedFont, setEmbedFont] = useState(false);
  const [thaiFontScale, setThaiFontScale] = useState(90);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  async function doApplyTm() {
    setApplyingTm(true);
    onNotice({ tone: "neutral", text: "Applying translation memory" });
    try {
      const n = await api.applyTm();
      onNotice({ tone: "success", text: `Filled ${n} from memory` });
      await refreshMeta();
      await reloadUnits();
    } catch (e) {
      onNotice({ tone: "error", text: String(e) });
    } finally {
      setApplyingTm(false);
    }
  }

  async function doExport() {
    setExporting(true);
    onNotice({ tone: "neutral", text: "Exporting" });
    try {
      const r = await api.exportProject(true, fontCapable && embedFont, renpyThai ? thaiFontScale : undefined);
      const text = (r.note ?? `Exported ${r.unitsApplied} units → ${r.filesWritten} files`) +
        (r.backupDir ? " (backup saved)" : "");
      // The text landed, but something the export promised didn't (a failed font
      // embed → the game renders boxes). Show it where failures go, not as part of
      // the success line.
      onNotice(r.warning
        ? { tone: "error", text: r.warning }
        : { tone: "success", text });
      await refreshMeta();
    } catch (e) {
      onNotice({ tone: "error", text: String(e) });
    } finally {
      setExporting(false);
    }
  }

  // Undo an in-place export: put the game's original files back from the
  // .rpgtl/source/ snapshots. Translations stay in the DB (re-export anytime).
  async function doRestore() {
    const ok = await ask(
      "Restore the original game files? This undoes your last export — the game goes back to its original (untranslated) state. Your translations are saved; you can export again anytime.",
      { title: "Restore original", kind: "warning" }
    );
    if (!ok) return;
    setRestoring(true);
    onNotice({ tone: "neutral", text: "Restoring" });
    try {
      const r = await api.restoreProject();
      onNotice({ tone: "success", text: r.note });
      await refreshMeta();
    } catch (e) {
      onNotice({ tone: "error", text: String(e) });
    } finally {
      setRestoring(false);
    }
  }

  async function doRescan() {
    setRescanning(true);
    onNotice({ tone: "neutral", text: "Rescanning" });
    try {
      const r = await api.rescanProject();
      await refreshMeta();
      await reloadUnits();
      const text = r.added > 0 || r.contextFilled > 0 || r.removed > 0
        ? `Rescanned: +${r.added} lines, ${r.contextFilled} speakers` +
          (r.removed > 0 ? `, ${r.removed} removed` : "")
        : "Rescanned: no changes";
      onNotice({ tone: "success", text });
    } catch (e) {
      onNotice({ tone: "error", text: String(e) });
    } finally {
      setRescanning(false);
    }
  }

  async function doCopySource() {
    const file = filter.file;
    if (!file) return;
    const name = file.split(/[\\/]/).pop() ?? file;
    const ok = await ask(
      `Copy source text into empty or failed translations in ${name}? Existing translations stay unchanged.`,
      { title: "Copy source?", kind: "info" }
    );
    if (!ok) return;
    onNotice({ tone: "neutral", text: `Copying source in ${name}` });
    try {
      const count = await api.copySourceToTranslation({ file });
      await refreshMeta();
      await reloadUnits();
      onNotice({ tone: "success", text: `Copied ${count} lines in ${name}` });
    } catch (e) {
      onNotice({ tone: "error", text: String(e) });
    }
  }

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
  const visibleStatusRows = statusRows.filter(
    (row) => row.status === undefined || row.count > 0 || filter.status === row.status
  );
  const toolsBusy = exporting || restoring || rescanning || applyingTm || unitsBusy;

  const allCount = files.reduce((a, f) => a + f.count, 0);

  // The overview owns the project identity; this compact label keeps the filter
  // rail focused on the game name rather than its long parent path.
  const sep = project.root.includes("\\") ? "\\" : "/";

  // Open the game folder in the OS file manager, showing its contents. openPath
  // opens the folder itself; if that command isn't available (an older build's
  // capability), fall back to revealing the project's own `.rpgtl/` sidecar — since
  // revealItemInDir opens the *parent* and selects the item, revealing a child of
  // the root lands Explorer *inside* the game folder (a permission this app has
  // always had, so it works without a rebuild).
  function openFolder() {
    openPath(project.root).catch(() =>
      revealItemInDir(`${project.root}${sep}.rpgtl`).catch(() =>
        revealItemInDir(project.root).catch(() => {})
      )
    );
  }

  return (
    <aside className="sidebar">
      <div className="sb-top">
        <button className="iconbtn sb-folder" onClick={openFolder} aria-label="Open game folder" title="Open folder">
          <Icon name="folder" />
        </button>
        <span className="sb-top-label">Project</span>
        <button
          className="iconbtn sb-collapse"
          onClick={onToggleCollapse}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          title={collapsed ? "Expand" : "Collapse"}
        >
          <Icon name={collapsed ? "chevron-right" : "chevron-left"} />
        </button>
      </div>

      <div className="sb-scroll">
        <div className="sb-section-title">Status</div>
        <div className="sb-list">
          {visibleStatusRows.map((r) => (
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

        {characters.length > 0 && (
          <>
            <div className="sb-section-title">Character</div>
            <select
              className="sb-select"
              aria-label="Filter by character"
              value={filter.context ?? ""}
              onChange={(e) => setFilter({ context: e.target.value || undefined })}
            >
              <option value="">All characters</option>
              {characters.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                </option>
              ))}
            </select>
          </>
        )}

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
              <span className="lbl">{f.file.split(/[\\/]/).pop() ?? f.file}</span>
              <span className="count">{f.count}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="sb-actions">
        <button className="primary full" onClick={doExport} disabled={exporting || restoring}>
          <Icon name="export" />
          <span className="lbl">{exporting ? "Exporting" : "Export"}</span>
        </button>
        <details className="sb-tools">
          <summary><Icon name="more" size={16} /><span className="lbl">Tools</span></summary>
          <div className="sb-tools-list">
            <button className="ghost" onClick={() => void doRescan()} disabled={toolsBusy}>
              <Icon name="retry" /> Rescan
            </button>
            <button className="ghost" onClick={doApplyTm} disabled={toolsBusy}>
              <Icon name="memory" /> Apply TM
            </button>
            <button
              className="ghost"
              onClick={() => void doCopySource()}
              disabled={!filter.file || toolsBusy}
              title={filter.file ? "Copy source into empty translations in this file" : "Select a file first"}
            >
              <Icon name="copy" /> Copy source
            </button>
            <button className="ghost" onClick={() => openPanel("lint")} disabled={toolsBusy}>
              <Icon name="lint" /> Lint
            </button>
            <button className="ghost" onClick={doRestore} disabled={toolsBusy}>
              <Icon name="restore" /> {restoring ? "Restoring" : "Restore"}
            </button>
            {renpyThai && !collapsed && (
              <label className="renpy-font-scale">
                Thai font
                <input type="number" min="70" max="120" step="1" value={thaiFontScale}
                  onChange={(e) => { const value = Number(e.target.value); if (Number.isInteger(value)) setThaiFontScale(Math.min(120, Math.max(70, value))); }}
                  disabled={exporting} aria-label="Thai font size percentage" /> %
              </label>
            )}
            {fontCapable && !collapsed && (
              <label className="chk embed-font-chk">
                <input type="checkbox" checked={embedFont} onChange={(e) => setEmbedFont(e.target.checked)} disabled={exporting} />
                Embed Thai font
              </label>
            )}
          </div>
        </details>
        <div className="sb-tools-bottom">
          <button className="iconbtn" onClick={toggleTheme} aria-label="Toggle light/dark theme" title="Toggle theme"><Icon name={theme === "dark" ? "sun" : "moon"} /></button>
          <button className="iconbtn" onClick={closeProject} aria-label="Close project" title="Close project"><Icon name="close" /></button>
          {version && <span className="sidebar-version">v{version}</span>}
        </div>
      </div>
    </aside>
  );
}
