import { useEffect, useState } from "react";
import { api, type ProviderKind } from "../ipc";
import { PROVIDER_LABELS, useSettings } from "../settings";

const KINDS: ProviderKind[] = ["local", "openai", "anthropic", "gemini", "openrouter"];

export default function SettingsView() {
  const s = useSettings();
  const active = s.active;
  const cfg = s.providers[active];
  const needsKey = active !== "local";

  const [keyInput, setKeyInput] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [test, setTest] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    setKeyInput("");
    setTest(null);
    if (needsKey) api.hasKey(active).then(setHasKey);
    else setHasKey(false);
  }, [active, needsKey]);

  async function saveKey() {
    if (!keyInput.trim()) return;
    await api.setKey(active, keyInput.trim());
    setKeyInput("");
    setHasKey(true);
  }
  async function clearKey() {
    await api.deleteKey(active);
    setHasKey(false);
  }
  async function runTest() {
    setTesting(true);
    setTest(null);
    try {
      const out = await api.testProvider(s.activeConfig());
      setTest(`✓ ${out}`);
    } catch (e) {
      setTest(`✗ ${String(e)}`);
    } finally {
      setTesting(false);
    }
  }

  return (
    <div className="settings">
      <div className="provider-tabs">
        {KINDS.map((k) => (
          <button
            key={k}
            className={k === active ? "tab active" : "tab"}
            onClick={() => s.setActive(k)}
          >
            {PROVIDER_LABELS[k]}
          </button>
        ))}
      </div>

      <div className="field-grid">
        <label>Model</label>
        <input
          value={cfg.model}
          onChange={(e) => s.updateProvider(active, { model: e.target.value })}
        />

        <label>Base URL</label>
        <input
          placeholder="(default)"
          value={cfg.baseUrl ?? ""}
          onChange={(e) => s.updateProvider(active, { baseUrl: e.target.value || undefined })}
        />

        <label>Temperature</label>
        <input
          type="number"
          step="0.1"
          min="0"
          max="2"
          value={cfg.temperature ?? 0.3}
          onChange={(e) => s.updateProvider(active, { temperature: Number(e.target.value) })}
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

        <label>Extra prompt</label>
        <textarea
          rows={3}
          placeholder="e.g. keep honorifics; the protagonist is female…"
          value={s.systemPrompt}
          onChange={(e) => s.setShared({ systemPrompt: e.target.value })}
        />
      </div>

      <div className="test-row">
        <button onClick={runTest} disabled={testing}>
          {testing ? "Testing…" : "Test connection"}
        </button>
        {test && <span className={test.startsWith("✓") ? "ok-msg" : "error"}>{test}</span>}
      </div>
    </div>
  );
}
