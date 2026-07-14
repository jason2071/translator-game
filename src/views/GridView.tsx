import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type TransUnit } from "../ipc";
import { useStore } from "../store";
import { UnitRow } from "../components/UnitRow";
import { Icon } from "../components/Icon";

export default function GridView() {
  const total = useStore((s) => s.total);
  const win = useStore((s) => s.window); // subscribe so a window fetch re-renders
  const ensureWindow = useStore((s) => s.ensureWindow);
  const loading = useStore((s) => s.loading);
  const setFilter = useStore((s) => s.setFilter);
  const filter = useStore((s) => s.filter);
  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 64,
    overscan: 14,
  });

  // Any filter change (search, character, file, status, untranslated) resets the
  // store window to offset 0; snap the virtualizer's DOM scroll to match so the
  // viewport shows the fresh result set instead of stale/blank rows. Skip the very
  // first render so opening a project doesn't force a scroll.
  const firstFilter = useRef(true);
  useEffect(() => {
    if (firstFilter.current) {
      firstFilter.current = false;
      return;
    }
    virtualizer.scrollToOffset(0);
  }, [filter]); // eslint-disable-line react-hooks/exhaustive-deps

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
            <p>
              {filter.search
                ? `No matches for "${filter.search}".`
                : "No units match the current filter."}
            </p>
            <button
              className="ghost"
              onClick={() =>
                setFilter({
                  file: undefined,
                  status: undefined,
                  search: undefined,
                  context: undefined,
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

type SearchField = "source" | "translation" | "context";

const SEARCH_FIELDS: { key: SearchField; label: string; word: string }[] = [
  { key: "source", label: "Source", word: "source" },
  { key: "translation", label: "Translation", word: "translation" },
  { key: "context", label: "Speaker", word: "speaker" },
];
const DEFAULT_FIELDS: SearchField[] = ["source", "translation"];
const SEARCH_DEBOUNCE_MS = 300;

function FilterBar() {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const total = useStore((s) => s.total);
  const loading = useStore((s) => s.loading);

  const [text, setText] = useState(filter.search ?? "");
  const [fields, setFields] = useState<SearchField[]>(
    (filter.searchFields as SearchField[] | undefined) ?? DEFAULT_FIELDS
  );
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const inputRef = useRef<HTMLInputElement>(null);

  // Re-sync when the query is cleared/changed elsewhere (e.g. "Reset filters"),
  // but keep local text if it already trims to the committed value (don't clobber
  // an in-progress trailing space).
  useEffect(() => {
    const incoming = filter.search ?? "";
    setText((cur) => (cur.trim() === incoming ? cur : incoming));
  }, [filter.search]);

  const cancelTimer = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = undefined;
  };
  const commit = (value: string, searchFields: SearchField[] = fields) => {
    cancelTimer();
    setFilter({ search: value.trim() || undefined, searchFields });
  };
  const onChange = (value: string) => {
    setText(value);
    cancelTimer();
    if (value.trim() === "") {
      // Empty commits immediately — covers the ✕ clear and backspace-to-empty.
      setFilter({ search: undefined, searchFields: fields });
      return;
    }
    timer.current = setTimeout(() => commit(value), SEARCH_DEBOUNCE_MS);
  };
  const clear = () => {
    cancelTimer();
    setText("");
    setFilter({ search: undefined, searchFields: fields });
    inputRef.current?.focus();
  };
  const toggleField = (key: SearchField) => {
    const active = fields.includes(key);
    if (active && fields.length === 1) return; // never zero fields
    const next = active ? fields.filter((f) => f !== key) : [...fields, key];
    setFields(next);
    commit(text, next); // re-run the current query against the new field set
  };

  const placeholder = "Search " + fields.map((f) => SEARCH_FIELDS.find((s) => s.key === f)!.word).join(" / ") + "…";

  return (
    <div className="searchbar">
      <div className="search-input-wrap">
        <Icon name="search" size={15} className="search-icon" />
        <input
          ref={inputRef}
          type="text"
          role="searchbox"
          aria-label="Search translation units"
          placeholder={placeholder}
          value={text}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit((e.target as HTMLInputElement).value);
            else if (e.key === "Escape") clear();
          }}
          onBlur={() => {
            if (timer.current) commit(text);
          }}
        />
        {text && (
          <button
            type="button"
            className="search-clear"
            aria-label="Clear search"
            title="Clear (Esc)"
            onClick={clear}
          >
            <Icon name="close" size={14} />
          </button>
        )}
      </div>

      <div className="field-toggle-group" role="group" aria-label="Search in">
        <span className="field-in">in</span>
        {SEARCH_FIELDS.map(({ key, label }) => {
          const active = fields.includes(key);
          const last = active && fields.length === 1;
          return (
            <button
              key={key}
              type="button"
              className={`field-toggle${active ? " active" : ""}`}
              aria-pressed={active}
              aria-disabled={last || undefined}
              title={last ? "At least one field must stay selected" : undefined}
              onClick={() => toggleField(key)}
            >
              {active && <Icon name="check" size={13} />}
              {label}
            </button>
          );
        })}
      </div>

      <label className="chk">
        <input
          type="checkbox"
          checked={!!filter.untranslatedOnly}
          onChange={(e) => setFilter({ untranslatedOnly: e.target.checked })}
        />
        Untranslated only
      </label>

      <span className="shown" role="status" aria-live="polite">
        {loading && <Icon name="retry" size={13} className="spin" />}
        {total.toLocaleString()} shown
      </span>
    </div>
  );
}

export type { TransUnit };
