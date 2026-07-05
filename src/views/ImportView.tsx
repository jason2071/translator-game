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
      <p className="subtitle">Import a game folder to begin</p>

      <button className="primary" onClick={pickFolder} disabled={checking || loading}>
        {checking ? "Detecting…" : "Choose game folder…"}
      </button>

      {path && <p className="path">{path}</p>}
      {(error || storeError) && <p className="error">{error || storeError}</p>}

      {detected && (
        <div className="detect-card">
          <div className="detect-row">
            <span>Engine</span>
            <strong>{detected.engineName}</strong>
          </div>
          <div className="detect-row">
            <span>Data files</span>
            <strong>{detected.fileCount}</strong>
          </div>
          <div className="detect-row">
            <span>Data dir</span>
            <code>{detected.dataDir}</code>
          </div>

          <div className="lang-pick">
            <label>
              From
              <select value={sourceLang} onChange={(e) => setSourceLang(e.target.value)}>
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
              <select value={targetLang} onChange={(e) => setTargetLang(e.target.value)}>
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

      {recents.length > 0 && (
        <section className="recent-section">
          <div className="recent-header">
            <h2 className="recent-title">Recent</h2>
            <button className="linklike" onClick={clearRecents} disabled={loading}>
              Clear
            </button>
          </div>
          <ul className="recent-list">
            {recents.map((r) => {
              const total = Math.max(r.stats.total, 1);
              const done = doneCount(r.stats);
              return (
                <li key={r.root} className="recent-item">
                  <button
                    className="recent-row"
                    disabled={loading}
                    onClick={() => reopenRecent(r.root)}
                  >
                    <Icon name="folder" className="recent-icon" />
                    <span className="recent-main">
                      <span className="recent-name" title={r.root}>
                        {pendingRoot === r.root ? "Opening…" : basename(r.root)}
                      </span>
                      <span className="recent-meta">
                        <span className="recent-badge">{r.engineName}</span>
                        <span>
                          {r.sourceLang} → {r.targetLang}
                        </span>
                        <span className="recent-progress">
                          <span className="recent-bar">
                            <span
                              className="recent-bar-fill"
                              style={{ width: `${(done / total) * 100}%` }}
                            />
                          </span>
                          <span className="recent-count">
                            {done}/{r.stats.total}
                          </span>
                        </span>
                        <span className="recent-time">
                          <Icon name="clock" size={12} /> {timeAgo(r.lastOpened)}
                        </span>
                      </span>
                    </span>
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
