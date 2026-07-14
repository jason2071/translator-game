import { useEffect, useRef, useState } from "react";
import { Icon, type IconName } from "./Icon";

export interface MenuItem {
  key: string;
  icon: IconName;
  label: string;
  onClick: () => void;
  title?: string;
}

// A small "⋯ more" overflow menu: an icon trigger plus a popover list of actions.
// Keeps a growing set of secondary/contextual toolbar buttons out of the primary
// row. Renders nothing when there are no items, so an empty menu never shows.
export function OverflowMenu({ items }: { items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // While open, close on an outside click or Escape; clean up on close/unmount.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (items.length === 0) return null;

  return (
    <div className="tb-menu" ref={ref}>
      <button
        type="button"
        className="ghost tb-menu-trigger"
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        title="More actions"
      >
        <Icon name="more" size={16} />
      </button>
      {open && (
        <div className="tb-menu-list" role="menu">
          {items.map((it) => (
            <button
              key={it.key}
              type="button"
              role="menuitem"
              className="tb-menu-item"
              title={it.title}
              onClick={() => {
                setOpen(false);
                it.onClick();
              }}
            >
              <Icon name={it.icon} size={14} /> {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
