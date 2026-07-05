import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, STATUSES, type Status, type TransUnit } from "../ipc";
import { useStore } from "../store";
import { useSettings } from "../settings";
import { useTranslation } from "../translation";
import { overflowLines } from "../messageWidth";
import { codesMismatch } from "../codes";
import { statusColor } from "../status";
import { Icon } from "./Icon";
import { MessagePreview } from "./MessagePreview";

// Kinds shown in a fixed-width message/choice box, where a too-long line
// overflows on screen. Other kinds (names, descriptions) have looser layouts.
const BOXED_KINDS = new Set(["Dialogue", "ScrollText", "Choice", "Message"]);

export const UnitRow = memo(function UnitRow({
  unit,
  index,
  onNext,
}: {
  unit: TransUnit;
  index?: number;
  onNext?: (nextIndex: number) => void;
}) {
  const editUnit = useStore((s) => s.editUnit);
  const setStatus = useStore((s) => s.setStatus);
  const engineId = useStore((s) => s.project?.engineId);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const maxLineWidth = useSettings((s) => s.maxLineWidth);
  const activeConfig = useSettings((s) => s.activeConfig);
  const enqueue = useTranslation((s) => s.enqueue);
  const unitsBusy = useTranslation((s) => s.units.phase !== "idle");
  const [value, setValue] = useState(unit.translation ?? "");
  const [showPreview, setShowPreview] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // Re-translate just this line via AI (overwrites the current translation). The
  // row fills live from the units-update event; refreshMeta updates the counts.
  async function retranslate() {
    setRetrying(true);
    try {
      await enqueue("units", () =>
        api.translateUnits({ ids: [unit.id], overwrite: true }, activeConfig())
      );
      await refreshMeta();
    } catch {
      // Transport errors surface via the global translate-error listener.
    } finally {
      setRetrying(false);
    }
  }

  // Keep local text in sync when the row is replaced (filter reload, AI fill).
  useEffect(() => {
    setValue(unit.translation ?? "");
  }, [unit.id, unit.translation]);

  // Grow the textarea to fit its content (the virtualizer's measureElement
  // ResizeObserver picks up the height change and re-lays out the row).
  useLayoutEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [value]);

  function commit() {
    if (value !== (unit.translation ?? "")) {
      editUnit(unit.id, value);
    }
  }

  // Warn against the text actually on screen (empty = no warning, handled inside).
  const warn = codesMismatch(unit.source, value, engineId);
  // Overflow guard: flag lines wider than the box, for boxed kinds only.
  const boxed = BOXED_KINDS.has(unit.kind);
  const overflow =
    maxLineWidth > 0 && boxed ? overflowLines(value, maxLineWidth, engineId) : [];

  return (
    <div className="unit-row" style={{ borderLeftColor: statusColor(unit.status) }}>
      <div className="cell source">
        <div className="src-text">{unit.source}</div>
        <div className="src-meta">
          <span className="file">{unit.file}</span>
          {unit.context && (
            <span className="ctx">
              <Icon name="speech" size={11} /> {unit.context}
            </span>
          )}
          <span className="kind">{unit.kind}</span>
        </div>
      </div>

      <div className="cell translation">
        <textarea
          ref={taRef}
          className={warn ? "warn" : ""}
          value={value}
          placeholder="—"
          spellCheck={false}
          rows={1}
          onChange={(e) => setValue(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            // Ctrl/Cmd+Enter: save and jump to the next row (plain Enter = newline).
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && index !== undefined) {
              e.preventDefault();
              commit();
              onNext?.(index + 1);
            }
          }}
        />
        {warn && (
          <span className="code-warn" title="Inline codes differ from the source">
            ⚠ codes differ
          </span>
        )}
        {overflow.length > 0 && (
          <span
            className="width-warn"
            title={overflow
              .map((o) => `line ${o.line}: ${o.width}/${maxLineWidth} wide`)
              .join("\n")}
          >
            ⚠ line too long ({overflow.map((o) => o.width).join(", ")}/{maxLineWidth})
          </span>
        )}
        {boxed && value && (
          <button
            type="button"
            className="preview-toggle"
            aria-pressed={showPreview}
            title="Preview in a message box"
            onClick={() => setShowPreview((p) => !p)}
          >
            <Icon name="eye" size={13} /> {showPreview ? "Hide preview" : "Preview"}
          </button>
        )}
        {boxed && showPreview && value && (
          <MessagePreview
            text={value}
            speaker={unit.kind === "Dialogue" ? unit.context : null}
            maxWidth={maxLineWidth}
            engineId={engineId}
          />
        )}
        <button
          type="button"
          className="row-retranslate"
          onClick={retranslate}
          disabled={unitsBusy || retrying}
          title="Re-translate this line with AI (overwrites the current translation)"
          aria-label="Re-translate this line with AI"
        >
          <Icon name="retry" size={13} className={retrying ? "spin" : undefined} />
        </button>
      </div>

      <div className="cell status">
        <span className="st-dot" style={{ background: statusColor(unit.status) }} />
        <select
          value={unit.status}
          onChange={(e) => setStatus(unit.id, e.target.value as Status)}
        >
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
});
