import { useState, useEffect, type CSSProperties } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
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
  const characters = useStore((s) => s.characters);
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const closeProject = useStore((s) => s.closeProject);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const reloadUnits = useStore((s) => s.reloadUnits);
  const theme = useTheme((s) => s.theme);
  const toggleTheme = useTheme((s) => s.toggle);

  const [exporting, setExporting] = useState(false);
  const [exportingMod, setExportingMod] = useState(false);
  const [applyingTm, setApplyingTm] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [version, setVersion] = useState("");

  // Stock game fonts often have no Thai glyphs, so offer to embed a Thai-capable font
  // on export. Default on when translating to Thai. Shown for the engines whose
  // `embed_font` does something (Ren'Py embeds its own font inside its tl/ path).
  const FONT_ENGINES = ["rpgmaker-mvmz", "unity-csvloc", "unity-textbl", "rpgmaker-hendrix"];
  // Engines whose translation can be packaged as a non-destructive overlay .zip.
  // Ren'Py + Hendrix build additively into the game, so they're export-to-game only.
  const MOD_ENGINES = [
    "unity-csvloc",
    "rpgmaker-mvmz",
    "godot",
    "tyrano",
    "kirikiri",
    "forger-acod",
    "ac-loctext",
  ];
  const fontCapable = FONT_ENGINES.includes(project.engineId);
  const modCapable = MOD_ENGINES.includes(project.engineId);
  const [embedFont, setEmbedFont] = useState(() => /thai/i.test(project.targetLang ?? ""));

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

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

  async function doExport() {
    setExporting(true);
    setErr(null);
    setResult(null);
    try {
      const r = await api.exportProject(true, fontCapable && embedFont);
      setResult(r);
      setMsg(r.note ?? `Exported ${r.unitsApplied} units → ${r.filesWritten} files`);
      await refreshMeta();
    } catch (e) {
      setErr(String(e));
    } finally {
      setExporting(false);
    }
  }

  // Export a distributable mod .zip that overlays onto the game (game untouched),
  // then reveal it in the file manager.
  async function doExportMod() {
    setExportingMod(true);
    setErr(null);
    setResult(null);
    try {
      const r = await api.exportMod(fontCapable && embedFont);
      setMsg((r.note ?? `Mod: ${r.unitsApplied} units → ${r.filesWritten} files`) + " (.zip)");
      await revealItemInDir(r.zipPath).catch(() => {});
    } catch (e) {
      setErr(String(e));
    } finally {
      setExportingMod(false);
    }
  }

  const pct =
    stats && stats.total > 0
      ? Math.round(((stats.total - stats.untranslated) / stats.total) * 100)
      : 0;
  const done = stats ? stats.translated + stats.reviewed : 0;
  const todo = stats?.untranslated ?? 0;
  const failed = stats?.failed ?? 0;

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

  // Show the game folder name in full, with a leading … standing in for the (long,
  // less useful) parent path — so the name is never clipped at its end.
  const pathSegs = project.root.split(/[\\/]/).filter(Boolean);
  const gameName = pathSegs[pathSegs.length - 1] ?? project.root;
  const sep = project.root.includes("\\") ? "\\" : "/";
  const shownPath = pathSegs.length > 1 ? `…${sep}${gameName}` : project.root;

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
        <div className="sb-title">
          <span className="sb-name">{project.engineName}</span>
          <span className="sb-path" title={project.root}>
            {shownPath}
          </span>
        </div>
        {!collapsed && (
          <button
            className="iconbtn"
            onClick={openFolder}
            aria-label="Open game folder in Explorer"
            title="Open game folder in Explorer"
          >
            <Icon name="folder" />
          </button>
        )}
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
          <span className="sb-stat done">
            <i className="dot" />
            <b>{done.toLocaleString()}</b> done
          </span>
          <span className="sb-stat todo">
            <i className="dot" />
            <b>{todo.toLocaleString()}</b> todo
          </span>
          {failed > 0 && (
            <button
              className="sb-stat failed"
              onClick={() => setFilter({ status: "Failed", untranslatedOnly: false })}
              title="Show the failed units"
            >
              <i className="dot" />
              <b>{failed.toLocaleString()}</b> failed
            </button>
          )}
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
        <button className="primary full" onClick={doExport} disabled={exporting || exportingMod}>
          <Icon name="export" />
          <span className="lbl">{exporting ? "Exporting…" : "Export → game"}</span>
        </button>
        {modCapable && (
          <button
            className="ghost full"
            onClick={doExportMod}
            disabled={exporting || exportingMod}
            title="Build a .zip you copy over the game (game untouched). It makes the game Thai with no in-game language switch."
          >
            <Icon name="export" />
            <span className="lbl">{exportingMod ? "Packaging…" : "Export as mod (.zip)"}</span>
          </button>
        )}
        <div className="row">
          {fontCapable && !collapsed && (
            <label className="chk embed-font-chk" title="Drop a Thai-capable font into the game and repoint its fonts at it, so translated Thai renders instead of missing-glyph boxes">
              <input
                type="checkbox"
                checked={embedFont}
                onChange={(e) => setEmbedFont(e.target.checked)}
                disabled={exporting || exportingMod}
              />
              Embed Thai font
            </label>
          )}
          <button className="iconbtn" onClick={toggleTheme} aria-label="Toggle light/dark theme" title="Toggle theme">
            <Icon name={theme === "dark" ? "sun" : "moon"} />
          </button>
          <button className="iconbtn" onClick={closeProject} aria-label="Close project" title="Close project">
            <Icon name="close" />
          </button>
        </div>
        {!collapsed && version && <p className="sidebar-version">v{version}</p>}
      </div>
    </aside>
  );
}
