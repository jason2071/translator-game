import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { api, type DetectResult } from "../ipc";
import { useStore } from "../store";
import { useRecents, timeAgo, basename, doneCount } from "../recents";
import { useTheme } from "../theme";
import { DEFAULT_SOURCE, DEFAULT_TARGET, SOURCE_LANGS, TARGET_LANGS } from "../langs";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import SettingsView from "./SettingsView";

export default function ImportView() {
  const openProject = useStore((s) => s.openProject);
  const loading = useStore((s) => s.loading);
  const storeError = useStore((s) => s.error);
  const recents = useRecents((s) => s.items);
  const removeRecent = useRecents((s) => s.remove);
  const clearRecents = useRecents((s) => s.clear);

  const [showSettings, setShowSettings] = useState(false);
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

  const shownRecents = recents.slice(0, 8);

  return (
    <div className="import-view">
      <header className="import-header">
        <div className="import-brand">
          <span className="import-brand-mark"><Icon name="folder" size={18} /></span>
          <span className="import-brand-copy">
            <strong>Game Translator</strong>
            <small>Game localization</small>
          </span>
        </div>
        <div className="import-topbar">
          {version && <span className="app-version">v{version}</span>}
          <button
            className="theme-fab iconbtn"
            onClick={() => setShowSettings(true)}
            title="Settings (AI providers, updates)"
            aria-label="Open settings"
          >
            <Icon name="settings" />
          </button>
          <button
            className="theme-fab iconbtn"
            onClick={toggleTheme}
            title={theme === "dark" ? "Switch to light" : "Switch to dark"}
            aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            <Icon name="theme" />
          </button>
        </div>
      </header>

      {showSettings && (
        <Modal title="Settings" onClose={() => setShowSettings(false)}>
          <SettingsView />
        </Modal>
      )}
      <main className={`import-content${detected ? " detect-mode" : recents.length === 0 ? " solo" : ""}`}>
        {!detected ? (
          <>
            <section className="import-open-card">
              <span className="import-open-icon"><Icon name="folder" size={24} /></span>
              <p className="import-kicker">New project</p>
              <h1>Translate a game</h1>
              <p className="subtitle">Open a supported game folder to begin.</p>
              <button className="primary" onClick={pickFolder} disabled={checking || loading}>
                {checking ? "Checking" : "Open folder"}
              </button>
              {(error || storeError) && (
                <p className="import-error-card">
                  <Icon name="warn" size={15} className="import-error-icon" />
                  <span>{error || storeError}</span>
                </p>
              )}
            </section>

            {recents.length > 0 && (
              <section className="recent-section">
                <div className="recent-header">
                  <div>
                    <p className="import-kicker">Workspace</p>
                    <h2 className="recent-title">Recent projects</h2>
                  </div>
                  <button className="linklike" onClick={clearRecents} disabled={loading}>
                    Clear
                  </button>
                </div>
                <ul className="recent-list">
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
                          <span className="recent-copy" title={r.root}>
                            <span className="recent-name">
                              {pendingRoot === r.root ? "Opening" : basename(r.root)}
                            </span>
                            <span className="recent-meta">
                              {r.engineName} · {r.sourceLang} → {r.targetLang}
                            </span>
                          </span>
                          <span className="recent-summary">
                            <span className={`recent-pct${done >= r.stats.total && r.stats.total > 0 ? " done" : ""}`}>
                              {Math.round((done / total) * 100)}%
                            </span>
                            <span className="recent-time">{timeAgo(r.lastOpened)}</span>
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
          </>
        ) : (
          <section className="import-detect-flow">
            <div className="import-detect-intro">
              <p className="import-kicker">Project setup</p>
              <h1>Check project</h1>
              <p className="subtitle">Review the engine and languages before opening.</p>
            </div>
            {(error || storeError) && (
              <p className="import-error-card">
                <Icon name="warn" size={15} className="import-error-icon" />
                <span>{error || storeError}</span>
              </p>
            )}
            <div className="detect-card">
              <div className="detect-head">
                <h2 className="detect-title">{path ? basename(path) : "Game"}</h2>
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
              <div className="detect-row detect-engine">
                <span>Engine</span>
                <strong>{detected.engineName}</strong>
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
                    {SOURCE_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
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
                    {TARGET_LANGS.map((l) => <option key={l} value={l}>{l}</option>)}
                  </select>
                </label>
              </div>

              <details className="detect-details">
                <summary>Details</summary>
                <div className="detect-row">
                  <span>Files</span>
                  <strong>{detected.fileCount}</strong>
                </div>
                <div className="detect-row detect-row-block">
                  <span>Data</span>
                  <code>{detected.dataDir}</code>
                </div>
                {path && <p className="path">{path}</p>}
              </details>

              <button
                className="primary"
                disabled={loading}
                onClick={() => path && openProject(path, sourceLang, targetLang)}
              >
                {loading ? "Opening" : "Open"}
              </button>
            </div>
          </section>
        )}
      </main>
    </div>
  );
}
