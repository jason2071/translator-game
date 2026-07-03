import type { Status } from "./ipc";

// Single source of truth for status → color, shared by the row accent/dot and
// the sidebar status filter list.
export function statusColor(s: Status): string {
  switch (s) {
    case "Untranslated":
      return "var(--muted)";
    case "Failed":
      return "var(--err)";
    case "Draft":
      return "var(--warn)";
    case "Translated":
      return "var(--status-translated)";
    case "Reviewed":
      return "var(--ok)";
    case "Locked":
      return "var(--lock)";
  }
}
