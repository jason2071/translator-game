import { useEffect, useRef, useState } from "react";
import { api, STATUSES, type Status, type TransUnit } from "../ipc";
import { useStore } from "../store";
import { useSettings } from "../settings";
import { useTranslation } from "../translation";
import { codesMismatch } from "../codes";
import { Icon } from "./Icon";

export function UnitInspector({
  unit,
  onNext,
  onClose,
}: {
  unit: TransUnit | undefined;
  onNext: () => void;
  onClose: () => void;
}) {
  const editUnit = useStore((s) => s.editUnit);
  const setStatus = useStore((s) => s.setStatus);
  const engineId = useStore((s) => s.project?.engineId);
  const refreshMeta = useStore((s) => s.refreshMeta);
  const activeConfig = useSettings((s) => s.activeConfig);
  const enqueue = useTranslation((s) => s.enqueue);
  const unitsBusy = useTranslation((s) => s.units.phase !== "idle");
  const inspectorRef = useRef<HTMLElement>(null);
  const draftRef = useRef<{ id: number; value: string; original: string } | null>(null);
  const mountedRef = useRef(true);
  const savesRef = useRef(new Map<number, { value: string; promise: Promise<boolean> }>());
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const previous = draftRef.current;
    if (previous && previous.id !== unit?.id && previous.value !== previous.original) {
      void persist(previous);
    }

    if (!unit) {
      draftRef.current = null;
      setValue("");
      return;
    }

    const incoming = unit.translation ?? "";
    if (!previous || previous.id !== unit.id) {
      draftRef.current = { id: unit.id, value: incoming, original: incoming };
      setValue(incoming);
      setError(null);
    } else if (previous.value === previous.original && previous.original !== incoming) {
      draftRef.current = { id: unit.id, value: incoming, original: incoming };
      setValue(incoming);
    }
  }, [unit?.id, unit?.translation]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      const draft = draftRef.current;
      if (draft && draft.value !== draft.original) {
        void persist(draft);
      }
    };
  }, [editUnit]);

  function updateValue(next: string) {
    setValue(next);
    if (draftRef.current) draftRef.current.value = next;
  }

  function persist(
    draft: { id: number; value: string; original: string },
  ): Promise<boolean> {
    if (draft.value === draft.original) return Promise.resolve(true);
    const existing = savesRef.current.get(draft.id);
    if (existing?.value === draft.value) return existing.promise;

    if (mountedRef.current) {
      setSaving(true);
      setError(null);
    }
    const savedValue = draft.value;
    const promise = editUnit(draft.id, savedValue)
      .then(() => {
        const current = draftRef.current;
        if (current?.id === draft.id && current.value === savedValue) {
          current.original = savedValue;
        }
        return true;
      })
      .catch((e) => {
        if (mountedRef.current) setError(`Save failed: ${String(e)}`);
        return false;
      })
      .finally(() => {
        const active = savesRef.current.get(draft.id);
        if (active?.promise === promise) savesRef.current.delete(draft.id);
        if (mountedRef.current) setSaving(savesRef.current.size > 0);
      });
    savesRef.current.set(draft.id, { value: savedValue, promise });
    return promise;
  }

  if (!unit) {
    return (
      <aside className="unit-inspector inspector-empty">
        <button className="iconbtn inspector-empty-close" onClick={onClose} aria-label="Close editor" title="Close">
          <Icon name="close" size={15} />
        </button>
        <Icon name="speech" size={22} />
        <p>Select a line</p>
      </aside>
    );
  }

  const selected = unit;
  const warn = codesMismatch(selected.source, value, engineId);

  function save() {
    const draft = draftRef.current;
    return draft ? persist(draft) : Promise.resolve(true);
  }

  async function retry() {
    setRetrying(true);
    setError(null);
    try {
      await enqueue("units", () =>
        api.translateUnits({ ids: [selected.id], overwrite: true }, activeConfig())
      );
      await refreshMeta();
    } catch (e) {
      setError(`Retry failed: ${String(e)}`);
    } finally {
      setRetrying(false);
    }
  }

  async function saveAndNext() {
    if (await save()) onNext();
  }

  return (
    <aside className="unit-inspector" ref={inspectorRef}>
      <div className="inspector-title">
        <span>Edit</span>
        <div className="inspector-title-actions">
          <select
            value={selected.status}
            onChange={(e) => {
              setError(null);
              void setStatus(selected.id, e.target.value as Status)
                .catch((err) => setError(`Status failed: ${String(err)}`));
            }}
            aria-label="Translation status"
          >
            {STATUSES.map((status) => <option key={status}>{status}</option>)}
          </select>
          <button className="iconbtn" onClick={onClose} aria-label="Close editor" title="Close">
            <Icon name="close" size={15} />
          </button>
        </div>
      </div>

      <p className="inspector-label">Original</p>
      <p className="inspector-source">{selected.source}</p>

      {selected.context && (
        <div className="inspector-context">
          <span><Icon name="speech" size={13} /> {selected.context}</span>
        </div>
      )}

      <label className="inspector-label" htmlFor={`translation-${selected.id}`}>Translation</label>
      <textarea
        id={`translation-${selected.id}`}
        className={warn ? "warn" : ""}
        value={value}
        onChange={(e) => updateValue(e.target.value)}
        onBlur={(e) => {
          const next = e.relatedTarget as Node | null;
          if (!next || !inspectorRef.current?.contains(next)) void save();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void saveAndNext();
          }
        }}
        spellCheck={false}
      />
      {warn && <p className="code-warn">Codes differ</p>}
      {error && <p className="inspector-error">{error}</p>}

      <div className="inspector-actions">
        <button className="ghost" onClick={() => updateValue(selected.source)}><Icon name="copy" size={14} /> Copy</button>
        <button className="ghost" onClick={() => void retry()} disabled={unitsBusy || retrying}><Icon name="retry" size={14} className={retrying ? "spin" : undefined} /> Retry</button>
        <button className="primary" onClick={() => void save()} disabled={saving}>{saving ? "Saving" : "Save"}</button>
        <button className="ghost" onClick={() => void saveAndNext()} disabled={saving}>Next</button>
      </div>
    </aside>
  );
}
