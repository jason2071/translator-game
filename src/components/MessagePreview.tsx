import { type ReactNode } from "react";
import { displayWidth } from "../messageWidth";

// A rough preview of how a translation renders in an RPGMaker message box:
// splits on newlines, interprets \C[n] color runs, shows \N[k]/\P[k]/\V[k] as
// name/var tokens, drops non-visual pacing/font codes, and marks lines that
// exceed the box width. Not a pixel-exact renderer — a sanity check for line
// breaks, colors, and overflow.

// RPGMaker default windowskin text colors 0..9 (approximate).
const RM_COLORS = [
  "#ffffff", // 0 normal
  "#40b0f0", // 1 blue
  "#ff6060", // 2 red
  "#50e070", // 3 green
  "#80d0ff", // 4 light blue
  "#c0a0ff", // 5 purple
  "#ffe070", // 6 yellow
  "#a0a0a0", // 7 gray
  "#e0e0e0", // 8 light gray
  "#ff9060", // 9 orange
];

// One token: a color code, a name/var/gold code, a pacing/font code to drop, an
// escaped backslash, or a run of plain text.
const TOKEN_RE =
  /\\C\[(\d+)\]|\\([NPV])\[(\d+)\]|\\G|\\FS\[\d+\]|\\\{|\\\}|\\[.|!^><]|\\\\|[^\\]+/g;

function renderLine(line: string): ReactNode[] {
  const spans: ReactNode[] = [];
  let color = RM_COLORS[0];
  let key = 0;
  TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = TOKEN_RE.exec(line))) {
    const tok = m[0];
    if (m[1] !== undefined) {
      color = RM_COLORS[Number(m[1])] ?? RM_COLORS[0]; // \C[n]
      continue;
    }
    if (m[2]) {
      // \N[k] / \P[k] actor name, \V[k] variable — value unknown at edit time.
      const label = m[2] === "V" ? "var" : "name";
      spans.push(
        <span key={key++} className="mp-token">
          [{label}]
        </span>
      );
      continue;
    }
    if (tok === "\\G") {
      spans.push(
        <span key={key++} className="mp-token">
          [G]
        </span>
      );
      continue;
    }
    if (tok === "\\\\") {
      spans.push(
        <span key={key++} style={{ color }}>
          {"\\"}
        </span>
      );
      continue;
    }
    if (tok.startsWith("\\")) continue; // pacing/font codes draw nothing
    spans.push(
      <span key={key++} style={{ color }}>
        {tok}
      </span>
    );
  }
  return spans;
}

export function MessagePreview({
  text,
  speaker,
  maxWidth,
  engineId,
}: {
  text: string;
  speaker?: string | null;
  maxWidth: number;
  engineId?: string | null;
}) {
  const lines = (text || "").split("\n");
  return (
    <div className="msg-preview">
      {speaker && <div className="mp-name">{speaker}</div>}
      <div
        className="mp-box"
        style={maxWidth > 0 ? { width: `${Math.max(maxWidth, 8)}ch` } : undefined}
      >
        {lines.map((ln, i) => {
          const over = maxWidth > 0 && displayWidth(ln, engineId) > maxWidth;
          return (
            <div key={i} className={over ? "mp-line over" : "mp-line"}>
              {ln === "" ? " " : renderLine(ln)}
            </div>
          );
        })}
      </div>
    </div>
  );
}
