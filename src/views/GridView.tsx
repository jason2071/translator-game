import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type TransUnit } from "../ipc";
import { useStore } from "../store";
import { UnitRow } from "../components/UnitRow";

export default function GridView() {
  const units = useStore((s) => s.units);
  const loading = useStore((s) => s.loading);
  const setFilter = useStore((s) => s.setFilter);
  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: units.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 64,
    overscan: 14,
  });

  // Ctrl/Cmd+Enter "save & next": the target row may be outside the mounted
  // window, so scroll it into view first, then retry focusing until it exists.
  function focusRowTextarea(index: number, tries = 8) {
    const el = parentRef.current?.querySelector<HTMLTextAreaElement>(
      `[data-index="${index}"] textarea`
    );
    if (el) {
      el.focus();
      const end = el.value.length;
      el.setSelectionRange(end, end);
      return;
    }
    if (tries > 0) requestAnimationFrame(() => focusRowTextarea(index, tries - 1));
  }
  function focusRow(index: number) {
    if (index < 0 || index >= units.length) return;
    virtualizer.scrollToIndex(index, { align: "center" });
    requestAnimationFrame(() => focusRowTextarea(index));
  }

  return (
    <div className="grid-wrap">
      <FilterBar />
      <div className="grid-head">
        <span>Source · context</span>
        <span>Translation</span>
        <span>Status</span>
      </div>
      <div className={`grid-scroll${loading ? " loading" : ""}`} ref={parentRef}>
        {units.length === 0 ? (
          <div className="empty">
            <p>No units match the current filter.</p>
            <button
              className="ghost"
              onClick={() =>
                setFilter({
                  file: undefined,
                  status: undefined,
                  search: undefined,
                  untranslatedOnly: false,
                })
              }
            >
              Reset filters
            </button>
          </div>
        ) : (
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((v) => {
              const unit = units[v.index];
              return (
                <div
                  key={unit.id}
                  ref={virtualizer.measureElement}
                  data-index={v.index}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${v.start}px)`,
                  }}
                >
                  <UnitRow unit={unit} index={v.index} onNext={focusRow} />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

function FilterBar() {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const units = useStore((s) => s.units);

  // File + status filters now live in the sidebar; this bar is search-only.
  return (
    <div className="searchbar">
      <input
        key={filter.search ?? ""}
        type="search"
        placeholder="Search source / translation… (Enter)"
        defaultValue={filter.search ?? ""}
        onKeyDown={(e) => {
          if (e.key === "Enter")
            setFilter({ search: (e.target as HTMLInputElement).value || undefined });
        }}
      />

      <label className="chk">
        <input
          type="checkbox"
          checked={!!filter.untranslatedOnly}
          onChange={(e) => setFilter({ untranslatedOnly: e.target.checked })}
        />
        Untranslated only
      </label>

      <span className="shown">{units.length} shown</span>
    </div>
  );
}

export type { TransUnit };
