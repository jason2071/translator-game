import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type DetectResult } from "../ipc";
import { useStore } from "../store";
import { DEFAULT_SOURCE, DEFAULT_TARGET, SOURCE_LANGS, TARGET_LANGS } from "../langs";

export default function ImportView() {
  const openProject = useStore((s) => s.openProject);
  const loading = useStore((s) => s.loading);
  const [path, setPath] = useState<string | null>(null);
  const [detected, setDetected] = useState<DetectResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sourceLang, setSourceLang] = useState<string>(DEFAULT_SOURCE);
  const [targetLang, setTargetLang] = useState<string>(DEFAULT_TARGET);

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

  return (
    <div className="import-view">
      <h1>RPGMaker Translator</h1>
      <p className="subtitle">Import a game folder to begin</p>

      <button className="primary" onClick={pickFolder} disabled={checking || loading}>
        {checking ? "Detecting…" : "Choose game folder…"}
      </button>

      {path && <p className="path">{path}</p>}
      {error && <p className="error">{error}</p>}

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
    </div>
  );
}
