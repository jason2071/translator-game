import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Icon } from "./Icon";

// Auto-checks for a newer published release on startup. If one exists, shows a
// banner; installing downloads only the update, applies it, and relaunches — no
// manual re-download. In dev / offline the check just fails and is ignored.
export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [busy, setBusy] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    check()
      .then((u) => {
        if (u?.available) setUpdate(u);
      })
      .catch(() => {
        /* no updater endpoint (dev) or offline — ignore */
      });
  }, []);

  if (!update || dismissed) return null;

  async function install() {
    if (!update) return;
    setBusy(true);
    setErr(null);
    try {
      await update.downloadAndInstall();
      await relaunch();
    } catch (e) {
      setErr(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="update-banner">
      <Icon name="sparkle" size={15} />
      <span className="msg">
        Update available — <b>v{update.version}</b>
      </span>
      {err && <span className="error">{err}</span>}
      <button className="primary" onClick={install} disabled={busy}>
        {busy ? "Installing…" : "Install & restart"}
      </button>
      <button
        className="iconbtn"
        onClick={() => setDismissed(true)}
        aria-label="Dismiss update"
        title="Later"
      >
        <Icon name="close" />
      </button>
    </div>
  );
}
