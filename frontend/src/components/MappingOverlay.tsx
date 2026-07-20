import type { PointerEvent as ReactPointerEvent } from "react";
import type { Mapping } from "../types";

type Props = {
  mappings: Mapping[];
  selectedId: string | null;
  editing: boolean;
  activeIds: ReadonlySet<number>;
  onSelect: (id: string) => void;
  onMove: (id: string, x: number, y: number) => void;
};

export function MappingOverlay({ mappings, selectedId, editing, activeIds, onSelect, onMove }: Props) {
  const startDrag = (event: ReactPointerEvent<HTMLButtonElement>, id: string) => {
    if (!editing) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    onSelect(id);
  };

  const moveDrag = (event: ReactPointerEvent<HTMLButtonElement>, id: string) => {
    if (!editing || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const bounds = event.currentTarget.parentElement?.getBoundingClientRect();
    if (!bounds) return;
    onMove(
      id,
      Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
      Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
    );
  };

  return (
    <div className={`mapping-overlay ${editing ? "is-editing" : ""}`}>
      {mappings.map((mapping) => (
        <button
          key={mapping.id}
          type="button"
          title={`${mapping.label} · Contact ${mapping.contactId}`}
          className={`mapping-node ${mapping.type} ${selectedId === mapping.id ? "selected" : ""} ${activeIds.has(mapping.contactId) ? "active" : ""}`}
          style={{
            left: `${mapping.x * 100}%`,
            top: `${mapping.y * 100}%`,
            width: mapping.type === "dpad" ? `${mapping.radius * 200}%` : undefined,
            aspectRatio: "1",
          }}
          onClick={() => editing && onSelect(mapping.id)}
          onPointerDown={(event) => startDrag(event, mapping.id)}
          onPointerMove={(event) => moveDrag(event, mapping.id)}
        >
          <span>{mapping.type === "dpad" ? "WASD" : mapping.key.replace("Key", "")}</span>
          <small>{mapping.contactId}</small>
        </button>
      ))}
    </div>
  );
}
