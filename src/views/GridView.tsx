import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type TransUnit } from "../ipc";
import { useStore } from "../store";
import { UnitRow } from "../components/UnitRow";
import { UnitInspector } from "../components/UnitInspector";
import { Icon } from "../components/Icon";

const PAGE_SIZES = [50, 100, 200] as const;
const DEFAULT_PAGE_SIZE = 100;
const PAGE_SIZE_KEY = "rpgtl:page-size";

function savedPageSize() {
  const value = Number(localStorage.getItem(PAGE_SIZE_KEY));
  return PAGE_SIZES.find((size) => size === value) ?? DEFAULT_PAGE_SIZE;
}

export default function GridView() {
  const total = useStore((s) => s.total);
  const win = useStore((s) => s.window); // subscribe so a window fetch re-renders
  const ensureWindow = useStore((s) => s.ensureWindow);
  const loading = useStore((s) => s.loading);
  const setFilter = useStore((s) => s.setFilter);
  const filter = useStore((s) => s.filter);
  const parentRef = useRef<HTMLDivElement>(null);
  const nextSelectionRef = useRef<number | null>(null);
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(savedPageSize);
  const [inspectorOpen, setInspectorOpen] = useState(
    () => localStorage.getItem("rpgtl:inspector-open") !== "false"
  );

  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const activePage = Math.min(page, pageCount - 1);
  const pageStart = activePage * pageSize;
  const pageRowCount = Math.max(0, Math.min(pageSize, total - pageStart));

  function openInspector(index: number) {
    setSelectedIndex(index);
    setInspectorOpen(true);
    localStorage.setItem("rpgtl:inspector-open", "true");
  }

  function closeInspector() {
    setInspectorOpen(false);
    localStorage.setItem("rpgtl:inspector-open", "false");
  }

  const virtualizer = useVirtualizer({
    count: pageRowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 44,
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
    setPage(0);
    setSelectedIndex(total > 0 ? 0 : null);
    virtualizer.scrollToOffset(0);
  }, [filter]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (page !== activePage) setPage(activePage);
  }, [activePage, page]);

  // Fetch the window around the visible range whenever it moves (the store
  // only refetches when we near the loaded slice's edge, so this is cheap).
  const items = virtualizer.getVirtualItems();
  const first = pageStart + (items.length ? items[0].index : 0);
  const last = pageStart + (items.length ? items[items.length - 1].index : 0);
  useEffect(() => {
    if (pageRowCount > 0) ensureWindow(first, last);
  }, [first, last, pageRowCount, ensureWindow]);

  useEffect(() => {
    if (win.rows.length === 0) return;
    const pending = nextSelectionRef.current;
    if (pending !== null && pending >= win.offset && pending < win.offset + win.rows.length) {
      nextSelectionRef.current = null;
      setSelectedIndex(pending);
      return;
    }
    const selectedInPage =
      selectedIndex !== null && selectedIndex >= pageStart && selectedIndex < pageStart + pageRowCount;
    const selectedInWindow =
      selectedIndex !== null && selectedIndex >= win.offset && selectedIndex < win.offset + win.rows.length;
    if ((!selectedInPage || !selectedInWindow) && pageRowCount > 0) setSelectedIndex(pageStart);
  }, [pageRowCount, pageStart, selectedIndex, win.offset, win.rows.length]);

  const selectedUnit =
    selectedIndex !== null && selectedIndex >= win.offset && selectedIndex < win.offset + win.rows.length
      ? win.rows[selectedIndex - win.offset]
      : undefined;

  function selectNext() {
    if (selectedIndex === null || selectedIndex >= total - 1) return;
    const next = selectedIndex + 1;
    const nextPage = Math.floor(next / pageSize);
    if (nextPage !== activePage) {
      nextSelectionRef.current = next;
      setPage(nextPage);
      setSelectedIndex(next);
      ensureWindow(next, next);
      virtualizer.scrollToOffset(0);
      return;
    }
    if (next >= win.offset && next < win.offset + win.rows.length) {
      setSelectedIndex(next);
      virtualizer.scrollToIndex(next - pageStart, { align: "center" });
      return;
    }
    nextSelectionRef.current = next;
    ensureWindow(next, next);
    virtualizer.scrollToIndex(next - pageStart, { align: "center" });
  }

  function goToPage(nextPage: number) {
    const target = Math.max(0, Math.min(nextPage, pageCount - 1));
    const targetStart = target * pageSize;
    setPage(target);
    setSelectedIndex(total > 0 ? targetStart : null);
    virtualizer.scrollToOffset(0);
    if (total > 0) ensureWindow(targetStart, Math.min(targetStart + pageSize - 1, total - 1));
  }

  function changePageSize(nextSize: number) {
    localStorage.setItem(PAGE_SIZE_KEY, String(nextSize));
    setPageSize(nextSize);
    setPage(0);
    setSelectedIndex(total > 0 ? 0 : null);
    virtualizer.scrollToOffset(0);
    if (total > 0) ensureWindow(0, Math.min(nextSize - 1, total - 1));
  }

  return (
    <div className={`translation-workspace${inspectorOpen ? " inspector-open" : ""}`}>
      <div className="grid-wrap">
        <FilterBar />
        <div className="grid-head">
          <span>Source</span>
          <span>Translation</span>
        </div>
        <div className={`grid-scroll${loading ? " loading" : ""}`} ref={parentRef}>
          {total === 0 ? (
            <div className="empty">
              <p>{filter.search ? `No matches for "${filter.search}".` : "No units match the current filter."}</p>
              <button className="ghost" onClick={() => setFilter({ file: undefined, status: undefined, search: undefined, matchCase: undefined, matchWholeWord: undefined, context: undefined, untranslatedOnly: false })}>Reset</button>
            </div>
          ) : (
            <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
              {items.map((v) => {
                const globalIndex = pageStart + v.index;
                const wi = globalIndex - win.offset;
                const unit = wi >= 0 && wi < win.rows.length ? win.rows[wi] : undefined;
                return (
                  <div key={v.key} ref={virtualizer.measureElement} data-index={v.index} style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${v.start}px)` }}>
                    {unit ? <UnitRow unit={unit} index={globalIndex} selected={globalIndex === selectedIndex} onSelect={(_, index) => openInspector(index)} /> : <div className="unit-row placeholder" aria-hidden><span className="ph-line" /></div>}
                  </div>
                );
              })}
            </div>
          )}
        </div>
        <Pagination
          page={activePage}
          pageCount={pageCount}
          pageSize={pageSize}
          total={total}
          onPageChange={goToPage}
          onPageSizeChange={changePageSize}
        />
      </div>
      {inspectorOpen && <UnitInspector unit={selectedUnit} onNext={selectNext} onClose={closeInspector} />}
    </div>
  );
}

function Pagination({
  page,
  pageCount,
  pageSize,
  total,
  onPageChange,
  onPageSizeChange,
}: {
  page: number;
  pageCount: number;
  pageSize: number;
  total: number;
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
}) {
  return (
    <nav className="grid-pagination" aria-label="Table pages">
      <label>
        Rows
        <select value={pageSize} onChange={(e) => onPageSizeChange(Number(e.target.value))}>
          {PAGE_SIZES.map((size) => <option key={size} value={size}>{size}</option>)}
        </select>
      </label>
      <div className="pagination-controls">
        <button className="ghost" onClick={() => onPageChange(page - 1)} disabled={page === 0 || total === 0}>Prev</button>
        <span aria-live="polite">Page {total === 0 ? 0 : page + 1} / {total === 0 ? 0 : pageCount}</span>
        <button className="ghost" onClick={() => onPageChange(page + 1)} disabled={page >= pageCount - 1 || total === 0}>Next</button>
      </div>
    </nav>
  );
}

const SEARCH_DEBOUNCE_MS = 300;

function FilterBar() {
  const filter = useStore((s) => s.filter);
  const setFilter = useStore((s) => s.setFilter);
  const total = useStore((s) => s.total);
  const loading = useStore((s) => s.loading);

  const [text, setText] = useState(filter.search ?? "");
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
  const commit = (value: string) => {
    cancelTimer();
    setFilter({ search: value.trim() || undefined, searchFields: undefined });
  };
  const onChange = (value: string) => {
    setText(value);
    cancelTimer();
    if (value.trim() === "") {
      // Empty commits immediately — covers the ✕ clear and backspace-to-empty.
      setFilter({ search: undefined, searchFields: undefined });
      return;
    }
    timer.current = setTimeout(() => commit(value), SEARCH_DEBOUNCE_MS);
  };
  const clear = () => {
    cancelTimer();
    setText("");
    setFilter({ search: undefined, searchFields: undefined });
    inputRef.current?.focus();
  };
  const toggleSearchOption = (key: "matchCase" | "matchWholeWord", checked: boolean) => {
    cancelTimer();
    setFilter({
      search: text.trim() || undefined,
      searchFields: undefined,
      [key]: checked,
    });
  };
  return (
    <div className="searchbar">
      <div className="search-input-wrap">
        <Icon name="search" size={15} className="search-icon" />
        <input
          ref={inputRef}
          type="text"
          role="searchbox"
          aria-label="Search translation units"
          placeholder="Search source / translation"
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

      <div className="search-options">
        <label className="chk"><input type="checkbox" checked={!!filter.matchCase} onChange={(e) => toggleSearchOption("matchCase", e.target.checked)} />Match case</label>
        <label className="chk"><input type="checkbox" checked={!!filter.matchWholeWord} onChange={(e) => toggleSearchOption("matchWholeWord", e.target.checked)} />Whole word</label>
      </div>

      <span className="shown" role="status" aria-live="polite">
        {loading && <Icon name="retry" size={13} className="spin" />}
        {total.toLocaleString()} shown
      </span>
    </div>
  );
}

export type { TransUnit };
