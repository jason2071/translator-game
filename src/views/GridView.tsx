import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type TransUnit } from "../ipc";
import { useStore } from "../store";
import { UnitRow } from "../components/UnitRow";

export default function GridView() {
  const total = useStore((s) => s.total);
  const win = useStore((s) => s.window); // subscribe so a window fetch re-renders
  const ensureWindow = useStore((s) => s.ensureWindow);
  const loading = useStore((s) => s.loading);
  const setFilter = useStore((s) => s.setFilter);
  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 64,
    overscan: 14,
  });

  // Fetch the window around the visible range whenever it moves (the store
  // only refetches when we near the loaded slice's edge, so this is cheap).
  const items = virtualizer.getVirtualItems();
  const first = items.length ? items[0].index : 0;
  const last = items.length ? items[items.length - 1].index : 0;
  useEffect(() => {
    if (total > 0) ensureWindow(first, last);
  }, [first, last, total, ensureWindow]);

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
    if (index < 0 || index >= total) return;
    ensureWindow(index, index); // make sure the target row's data is (being) fetched
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
        {total === 0 ? (
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
            {items.map((v) => {
              const wi = v.index - win.offset;
              const unit = wi >= 0 && wi < win.rows.length ? win.rows[wi] : undefined;
              return (
                <div
                  key={v.key}
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
                  {unit ? (
                    <UnitRow unit={unit} index={v.index} onNext={focusRow} />
                  ) : (
                    <div className="unit-row placeholder" aria-hidden>
                      <span className="ph-line" />
                    </div>
                  )}
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
  const total = useStore((s) => s.total);

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

      <span className="shown">{total} shown</span>
    </div>
  );
}

export type { TransUnit };
