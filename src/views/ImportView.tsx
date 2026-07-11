import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { api, type DetectResult } from "../ipc";
import { useStore } from "../store";
import { useRecents, timeAgo, basename, doneCount } from "../recents";
import { useTheme } from "../theme";
import { DEFAULT_SOURCE, DEFAULT_TARGET, SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { Icon } from "../components/Icon";

export default function ImportView() {
  const openProject = useStore((s) => s.openProject);
  const loading = useStore((s) => s.loading);
  const storeError = useStore((s) => s.error);
  const recents = useRecents((s) => s.items);
  const removeRecent = useRecents((s) => s.remove);
  const clearRecents = useRecents((s) => s.clear);

  const [path, setPath] = useState<string | null>(null);
  const [detected, setDetected] = useState<DetectResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sourceLang, setSourceLang] = useState<string>(DEFAULT_SOURCE);
  const [targetLang, setTargetLang] = useState<string>(DEFAULT_TARGET);
  const [pendingRoot, setPendingRoot] = useState<string | null>(null);
  const [failedRoot, setFailedRoot] = useState<string | null>(null);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  async function pickFolder() {
    setError(null);
    setPath(null);
    setDetected(null);
    const picked = await open({ directory: true, title: "Select game folder" });
    if (typeof picked !== "string") return;
    setPath(picked);
    setChecking(true);
    setDetected(null);
    try {
      const res = await api.detectGame(picked);
      if (!res) {
        setError("No supported game engine detected in this folder.");
      } else {
        setDetected(res);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  }

  // Dismiss a pending folder selection and return to the recents view, without
  // reopening the OS folder dialog (which is the only other way out of a selection).
  function resetSelection() {
    setPath(null);
    setDetected(null);
    setError(null);
    setChecking(false);
  }

  // Reopen a project straight from history — no detect step or language pickers:
  // the backend keeps its saved languages and doesn't re-extract.
  async function reopenRecent(root: string) {
    setFailedRoot(null);
    setPendingRoot(root);
    await openProject(root); // never rethrows; sets store.error on failure
    setPendingRoot(null);
    if (useStore.getState().error) setFailedRoot(root);
  }

  const theme = useTheme((s) => s.theme);
  const toggleTheme = useTheme((s) => s.toggle);

  // Show a fixed cap of recents in a column-first 2-column grid (newest top-left).
  // The row count is balanced (≤10 rows → ≤10 per column) so the left column fills
  // first and the grid stays even for any count.
  const shownRecents = recents.slice(0, 20);
  const recentRows = Math.min(10, Math.ceil(shownRecents.length / 2));

  return (
    <div className="import-view">
      <div className="import-topbar">
        {version && <span className="app-version">v{version}</span>}
        <button
          className="theme-fab iconbtn"
          onClick={toggleTheme}
          title="Toggle theme"
          aria-label="Toggle light/dark theme"
        >
          <Icon name={theme === "dark" ? "sun" : "moon"} />
        </button>
      </div>
      <h1>RPGMaker Translator</h1>
      <p className="subtitle">
        {detected
          ? "Review the detected engine and languages, then open"
          : "Import a game folder to begin"}
      </p>

      <button className="primary" onClick={pickFolder} disabled={checking || loading}>
        {checking ? "Detecting…" : "Choose game folder…"}
      </button>

      {path && <p className="path">{path}</p>}
      {(error || storeError) && (
        <p className="import-error-card">
          <Icon name="warn" size={15} className="import-error-icon" />
          <span>{error || storeError}</span>
        </p>
      )}

      {detected && (
        <div className="detect-card">
          <div className="detect-head">
            <h2 className="detect-title">Detected game</h2>
            <button
              className="detect-dismiss iconbtn"
              onClick={resetSelection}
              disabled={loading}
              aria-label="Cancel — choose a different folder"
              title="Cancel"
            >
              <Icon name="close" size={14} />
            </button>
          </div>
          <div className="detect-row">
            <span>Engine</span>
            <strong>{detected.engineName}</strong>
          </div>
          <div className="detect-row">
            <span>Data files</span>
            <strong>{detected.fileCount}</strong>
          </div>
          <div className="detect-row detect-row-block">
            <span>Data dir</span>
            <code>{detected.dataDir}</code>
          </div>

          {detected.warnings?.map((w, i) => (
            <p key={i} className="detect-warning">
              <Icon name="warn" size={15} className="detect-warning-icon" />
              <span>{w}</span>
            </p>
          ))}

          <div className="lang-pick">
            <label>
              From
              <select
                value={sourceLang}
                disabled={loading}
                onChange={(e) => setSourceLang(e.target.value)}
              >
                {SOURCE_LANGS.map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
            <span className="arrow">→</span>
            <label>
              To
              <select
                value={targetLang}
                disabled={loading}
                onChange={(e) => setTargetLang(e.target.value)}
              >
                {TARGET_LANGS.map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <button
            className="primary"
            disabled={loading}
            onClick={() => path && openProject(path, sourceLang, targetLang)}
          >
            {loading ? "Extracting…" : "Open project"}
          </button>
        </div>
      )}

      {recents.length > 0 && !detected && (
        <section className="recent-section">
          <div className="recent-header">
            <h2 className="recent-title">Recent</h2>
            <button className="linklike" onClick={clearRecents} disabled={loading}>
              Clear
            </button>
          </div>
          <ul
            className="recent-list"
            style={{ gridTemplateRows: `repeat(${recentRows}, auto)` }}
          >
            {shownRecents.map((r) => {
              const total = Math.max(r.stats.total, 1);
              const done = doneCount(r.stats);
              return (
                <li key={r.root} className="recent-item">
                  <button
                    className="recent-row"
                    disabled={loading}
                    aria-label={`${basename(r.root)} — ${Math.round((done / total) * 100)}% translated, opened ${timeAgo(r.lastOpened)}`}
                    onClick={() => reopenRecent(r.root)}
                  >
                    <Icon name="folder" size={18} className="recent-icon" />
                    <span className="recent-name" title={r.root}>
                      {pendingRoot === r.root ? "Opening…" : basename(r.root)}
                    </span>
                    <span className="recent-progress" aria-hidden="true">
                      <span className="recent-bar">
                        <span
                          className="recent-bar-fill"
                          style={{ width: `${(done / total) * 100}%` }}
                        />
                      </span>
                      <span className={`recent-pct${done >= r.stats.total && r.stats.total > 0 ? " done" : ""}`}>
                        {Math.round((done / total) * 100)}%
                      </span>
                    </span>
                    <span className="recent-time">{timeAgo(r.lastOpened)}</span>
                  </button>

                  <button
                    className="recent-remove iconbtn"
                    disabled={loading}
                    aria-label={`Remove ${basename(r.root)} from recent projects`}
                    onClick={() => removeRecent(r.root)}
                  >
                    <Icon name="close" size={14} />
                  </button>

                  {failedRoot === r.root && (
                    <p className="recent-error">
                      Couldn't open — the folder may have moved or been deleted.{" "}
                      <button className="linklike" onClick={() => removeRecent(r.root)}>
                        Remove
                      </button>
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      )}
    </div>
  );
}
