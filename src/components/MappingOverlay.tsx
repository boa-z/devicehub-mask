import type { PointerEvent as ReactPointerEvent } from "react";
import { mappingBindingLabel, mappingContactIds, mappingLabel, mappingPosition, type Mapping } from "../types";

type Props = { mappings: Mapping[]; selectedId: string | null; editing: boolean; showGuides: boolean; frameSize: { width: number; height: number }; activeMappingIds: ReadonlySet<string>; onSelect: (id: string) => void; onMove: (id: string, x: number, y: number) => void };

function guidePoints(mapping: Mapping) {
  if (mapping.type === "Swipe") return mapping.positions;
  if (mapping.type === "MultipleTap") return mapping.items.map((item) => item.position);
  return [];
}

function guideEllipse(mapping: Mapping, frameSize: { width: number; height: number }) {
  const center = mapping.type === "MouseCastSpell" ? mapping.center : mappingPosition(mapping);
  const radii = mapping.type === "dpad"
    ? { x: mapping.radius, y: mapping.radius }
    : mapping.type === "DirectionPad"
      ? { x: mapping.max_offset_x / frameSize.width, y: mapping.max_offset_y / frameSize.height }
      : mapping.type === "MouseCastSpell"
        ? { x: mapping.cast_radius / frameSize.width, y: mapping.cast_radius / frameSize.height }
        : mapping.type === "PadCastSpell"
          ? { x: mapping.drag_radius / frameSize.width, y: mapping.drag_radius / frameSize.height }
          : mapping.type === "Observation" && mapping.max_radius > 0
            ? { x: mapping.max_radius / frameSize.width, y: mapping.max_radius / frameSize.height }
            : null;
  return radii ? { center, radii } : null;
}

export function MappingOverlay({ mappings, selectedId, editing, showGuides, frameSize, activeMappingIds, onSelect, onMove }: Props) {
  const guideHeight = 100;
  const guideWidth = frameSize.width > 0 && frameSize.height > 0 ? guideHeight * frameSize.width / frameSize.height : guideHeight;
  const moveDrag = (event: ReactPointerEvent<HTMLButtonElement>, id: string) => { if (!editing || !event.currentTarget.hasPointerCapture(event.pointerId)) return; const bounds = event.currentTarget.parentElement?.getBoundingClientRect(); if (bounds) onMove(id, Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)), Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height))); };
  return <div className={`mapping-overlay ${editing ? "is-editing" : ""}`}>
    <svg className="mapping-guides" viewBox={`0 0 ${guideWidth} ${guideHeight}`} aria-hidden="true">
      {mappings.map((mapping) => {
        if (!showGuides && mapping.id !== selectedId) return null;
        const points = guidePoints(mapping);
        const ellipse = guideEllipse(mapping, frameSize);
        return <g key={mapping.id} className={mapping.id === selectedId ? "selected" : ""}>
          {ellipse && <ellipse cx={ellipse.center.x * guideWidth} cy={ellipse.center.y * guideHeight} rx={ellipse.radii.x * guideWidth} ry={ellipse.radii.y * guideHeight} />}
          {points.length > 1 && <polyline points={points.map((point) => `${point.x * guideWidth},${point.y * guideHeight}`).join(" ")} />}
          {points.map((point, index) => <g key={index} className="mapping-guide-point"><circle cx={point.x * guideWidth} cy={point.y * guideHeight} r="1.3" /><text x={point.x * guideWidth} y={point.y * guideHeight - 2}>{index + 1}</text></g>)}
        </g>;
      })}
    </svg>
    {mappings.map((mapping) => { const position = mappingPosition(mapping); const ids = mappingContactIds(mapping); const radius = mapping.type === "dpad" ? mapping.radius : undefined; const binding = mappingBindingLabel(mapping); return <button key={mapping.id} type="button" title={`${mappingLabel(mapping)} · ${binding ?? mapping.type}`} className={`mapping-node ${mapping.type} ${selectedId === mapping.id ? "selected" : ""} ${activeMappingIds.has(mapping.id) ? "active" : ""}`} style={{ left: `${position.x * 100}%`, top: `${position.y * 100}%`, width: radius ? `${radius * 200}%` : undefined, aspectRatio: "1" }} onClick={() => editing && onSelect(mapping.id)} onPointerDown={(event) => { if (editing) { event.currentTarget.setPointerCapture(event.pointerId); onSelect(mapping.id); } }} onPointerMove={(event) => moveDrag(event, mapping.id)}><span>{binding ?? mapping.type.replace(/([a-z])([A-Z])/g, "$1 $2")}</span><small>{ids.join("/") || "-"}</small></button>; })}
  </div>;
}
