import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdate } from "../update";
import { api, type ProviderKind } from "../ipc";
import { PROVIDER_LABELS, PROVIDER_KINDS, useSettings } from "../settings";
import { Icon } from "../components/Icon";

export default function SettingsView() {
  const s = useSettings();
  // Which provider this modal is *configuring* — independent of the active (Run)
  // provider, and opened on it. Switching tabs here never changes the Run choice.
  const [editing, setEditing] = useState<ProviderKind>(s.active);
  const cfg = s.providers[editing];
  const needsKey = editing !== "local";

  const [keyInput, setKeyInput] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [test, setTest] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsErr, setModelsErr] = useState<string | null>(null);

  useEffect(() => {
    setKeyInput("");
    setTest(null);
    setModels([]);
    setModelsErr(null);
    if (needsKey) api.hasKey(editing).then(setHasKey);
    else setHasKey(false);
  }, [editing, needsKey]);

  async function refreshModels() {
    setLoadingModels(true);
    setModelsErr(null);
    try {
      const list = await api.listModels(s.configFor(editing));
      setModels(list);
      if (list.length === 0) setModelsErr("No models returned.");
    } catch (e) {
      setModelsErr(String(e));
    } finally {
      setLoadingModels(false);
    }
  }

  async function saveKey() {
    if (!keyInput.trim()) return;
    await api.setKey(editing, keyInput.trim());
    setKeyInput("");
    setHasKey(true);
  }
  async function clearKey() {
    await api.deleteKey(editing);
    setHasKey(false);
  }
  async function runTest() {
    setTesting(true);
    setTest(null);
    try {
      const out = await api.testProvider(s.configFor(editing));
      setTest(`✓ ${out}`);
    } catch (e) {
      setTest(`✗ ${String(e)}`);
    } finally {
      setTesting(false);
    }
  }

  // Updates: the app also auto-checks on startup (see UpdateBanner); this is a
  // manual re-check the user can trigger, e.g. right after a release. Uses the
  // GitHub Releases API and opens the download page (no auto-install).
  const [version, setVersion] = useState("");
  const [upState, setUpState] = useState<"idle" | "checking" | "latest" | "avail" | "error">("idle");
  const [upMsg, setUpMsg] = useState("");
  const [upUrl, setUpUrl] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  async function checkUpdate() {
    setUpState("checking");
    setUpMsg("");
    setUpUrl("");
    try {
      const info = await checkForUpdate();
      if (info) {
        setUpUrl(info.url);
        setUpState("avail");
        setUpMsg(`v${info.version} available`);
      } else {
        setUpState("latest");
        setUpMsg("You're on the latest version.");
      }
    } catch (e) {
      setUpState("error");
      setUpMsg(`Couldn't check: ${String(e)}`);
    }
  }

  return (
    <div className="settings">
      <div className="provider-tabs">
        {PROVIDER_KINDS.map((k) => (
          <button
            key={k}
            className={k === editing ? "tab active" : "tab"}
            onClick={() => setEditing(k)}
          >
            {PROVIDER_LABELS[k]}
          </button>
        ))}
      </div>

      <div className="field-grid">
        <label>Model</label>
        <div className="model-row">
          <input
            placeholder="model id"
            value={cfg.model}
            onChange={(e) => s.updateProvider(editing, { model: e.target.value })}
          />
          <button
            className="ghost"
            onClick={refreshModels}
            disabled={loadingModels}
            style={{ display: "inline-flex", alignItems: "center", gap: "0.3rem" }}
          >
            <Icon name="retry" size={14} /> {loadingModels ? "…" : "Refresh"}
          </button>
        </div>

        {models.length > 0 && (
          <>
            <span />
            <select
              className="model-select"
              value={models.includes(cfg.model) ? cfg.model : ""}
              onChange={(e) =>
                e.target.value && s.updateProvider(editing, { model: e.target.value })
              }
            >
              <option value="">— pick one of {models.length} installed —</option>
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </>
        )}
        {modelsErr && (
          <>
            <span />
            <span className="error models-hint">{modelsErr}</span>
          </>
        )}

        <label>Base URL</label>
        <input
          placeholder="(default)"
          value={cfg.baseUrl ?? ""}
          onChange={(e) => s.updateProvider(editing, { baseUrl: e.target.value || undefined })}
        />

        <label>Temperature</label>
        <input
          type="number"
          step="0.1"
          min="0"
          max="2"
          value={cfg.temperature ?? 0.3}
          onChange={(e) => s.updateProvider(editing, { temperature: Number(e.target.value) })}
        />

        {needsKey && (
          <>
            <label>API key</label>
            <div className="key-row">
              <input
                type="password"
                placeholder={hasKey ? "•••••••• (stored)" : "paste key…"}
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
              />
              <button className="primary" onClick={saveKey}>
                Save
              </button>
              {hasKey && (
                <button className="ghost" onClick={clearKey}>
                  Clear
                </button>
              )}
            </div>
          </>
        )}
      </div>

      <hr />
      <h3>Shared</h3>
      <div className="field-grid">
        <label>Target tone</label>
        <input value={s.tone} onChange={(e) => s.setShared({ tone: e.target.value })} />

        <label>Batch size</label>
        <input
          type="number"
          min="1"
          max="200"
          value={s.batchSize}
          onChange={(e) => s.setShared({ batchSize: Number(e.target.value) })}
        />

        <label>Rate limit (req/min, 0 = off)</label>
        <input
          type="number"
          min="0"
          value={s.rpm}
          onChange={(e) => s.setShared({ rpm: Number(e.target.value) })}
        />

        <label>Message width guard (chars, 0 = off)</label>
        <input
          type="number"
          min="0"
          max="120"
          value={s.maxLineWidth}
          onChange={(e) => s.setShared({ maxLineWidth: Number(e.target.value) })}
        />

        <label>Thinking / reasoning</label>
        <label className="chk">
          <input
            type="checkbox"
            checked={s.thinking}
            onChange={(e) => s.setShared({ thinking: e.target.checked })}
          />
          {s.thinking ? "On — slower, may improve quality" : "Off — faster, recommended for translation"}
        </label>

        <label className="label-top">
          Extra prompt <span className="hint">(all projects)</span>
        </label>
        <div className="prompt-field">
          <textarea
            rows={4}
            placeholder="Applies to every game. e.g. keep honorifics; the protagonist is a boy…"
            value={s.systemPrompt}
            onChange={(e) => s.setShared({ systemPrompt: e.target.value })}
          />
          <button
            type="button"
            className="btn-reset prompt-reset"
            onClick={() => s.resetSystemPrompt()}
            title="Refill with the bundled default translation prompt"
          >
            <Icon name="retry" size={13} /> Reset to default
          </button>
        </div>
      </div>

      <div className="test-row">
        <button onClick={runTest} disabled={testing}>
          {testing ? "Testing…" : "Test connection"}
        </button>
        {test && <span className={test.startsWith("✓") ? "ok-msg" : "error"}>{test}</span>}
      </div>

      <hr />
      <div className="update-row">
        <button
          className="btn-reset"
          onClick={checkUpdate}
          disabled={upState === "checking"}
          title="Check GitHub for a newer release"
        >
          <Icon name="retry" size={13} className={upState === "checking" ? "spin" : undefined} />
          {upState === "checking" ? "Checking…" : "Check for updates"}
        </button>
        {version && <span className="hint">Current: v{version}</span>}
        {upMsg && (
          <span className={upState === "error" ? "error" : upState === "avail" ? "ok-msg" : "hint"}>
            {upMsg}
          </span>
        )}
        {upState === "avail" && upUrl && (
          <button className="primary" onClick={() => openUrl(upUrl).catch(() => {})}>
            Open download page
          </button>
        )}
      </div>
    </div>
  );
}
