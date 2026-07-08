import { useEffect, useId, useRef, type ReactNode } from "react";
import { Icon } from "./Icon";

export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();

  // Move focus into the dialog on open, restore it to the opener on close.
  // MOUNT-ONLY (deps `[]`): this must not re-run on every render. Parents pass a
  // fresh inline `onClose` each render, and many also subscribe to store state
  // (e.g. App → `project`), so a controlled input inside the dialog would, on
  // every keystroke, re-render the parent → new `onClose` → this effect re-runs →
  // focus is yanked back to the dialog's first focusable element mid-typing. The
  // Escape handler lives in its own effect below so it can depend on `onClose`.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current
      ?.querySelector<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
      )
      ?.focus();
    return () => {
      prev?.focus?.(); // restore focus to whatever opened the dialog
    };
  }, []);

  // Escape closes. Re-binds when `onClose` changes — harmless (only swaps a
  // keydown listener; no focus side effects).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal"
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
      >
        <header className="modal-head">
          <h2 id={titleId}>{title}</h2>
          <button className="iconbtn" onClick={onClose} aria-label="Close dialog" title="Close">
            <Icon name="close" />
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
