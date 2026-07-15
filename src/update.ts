import { getVersion } from "@tauri-apps/api/app";
import { api } from "./ipc";

// Update checking via the GitHub Releases API (no signed updater manifest needed —
// works against the normal releases). We surface a newer version and open its
// download page rather than auto-installing.

/** True if `latest` (e.g. "0.12.14") is a higher version than `current`. */
export function isNewer(latest: string, current: string): boolean {
  const a = latest.split(".").map((n) => parseInt(n, 10) || 0);
  const b = current.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = a[i] ?? 0;
    const y = b[i] ?? 0;
    if (x !== y) return x > y;
  }
  return false;
}

export interface UpdateInfo {
  version: string;
  url: string;
  current: string;
}

/** Query the latest GitHub release; returns info only when it's newer than the
 *  running app, else null. Throws on a network/API error. */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  const [rel, current] = await Promise.all([api.latestRelease(), getVersion()]);
  return isNewer(rel.version, current)
    ? { version: rel.version, url: rel.url, current }
    : null;
}
