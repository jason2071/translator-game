import { memo } from "react";
import type { TransUnit } from "../ipc";

export const UnitRow = memo(function UnitRow({
  unit,
  index,
  selected,
  onSelect,
}: {
  unit: TransUnit;
  index: number;
  selected: boolean;
  onSelect: (unit: TransUnit, index: number) => void;
}) {
  const translation = unit.translation?.trim() || "—";

  return (
    <button
      type="button"
      className={`unit-row${selected ? " selected" : ""}`}
      onClick={() => onSelect(unit, index)}
      aria-pressed={selected}
    >
      <span className="cell source">
        <span className="src-text">{unit.source}</span>
        {unit.context && <span className="row-context">{unit.context}</span>}
      </span>
      <span className={`cell row-translation${translation === "—" ? " is-empty" : ""}`}>
        {translation}
      </span>
    </button>
  );
});
