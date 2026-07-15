import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { checkForUpdate, type UpdateInfo } from "../update";
import { Icon } from "./Icon";

// Auto-checks GitHub for a newer published release on startup. If one exists, shows a
// banner linking to its download page. In dev / offline the check just fails and is
// ignored. (No signed updater manifest needed — a plain Releases-API check.)
export function UpdateBanner() {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    checkForUpdate()
      .then((u) => {
        if (u) setUpdate(u);
      })
      .catch(() => {
        /* offline or API error — ignore */
      });
  }, []);

  if (!update || dismissed) return null;

  return (
    <div className="update-banner">
      <Icon name="sparkle" size={15} />
      <span className="msg">
        Update available — <b>v{update.version}</b>
      </span>
      <button className="primary" onClick={() => openUrl(update.url).catch(() => {})}>
        Open download page
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
