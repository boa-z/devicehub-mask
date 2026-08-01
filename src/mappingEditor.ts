import { createMapping, mappingContactIds, mappingPosition, type ButtonBinding, type DirectionBinding, type KeyMapping, type KeyMappingType, type Mapping, type Position } from "./types";

const CONTACT_IDS = [0, 1, 2, 3, 4] as const;

function cloneValue<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function nextContactId(mappings: Mapping[], reserved: number[] = []) {
  const usage = new Map<number, number>(CONTACT_IDS.map((id) => [id, 0]));
  for (const mapping of mappings) {
    for (const id of mappingContactIds(mapping)) {
      if (usage.has(id)) usage.set(id, (usage.get(id) ?? 0) + 1);
    }
  }
  return CONTACT_IDS
    .filter((id) => !reserved.includes(id))
    .sort((left, right) => (usage.get(left) ?? 0) - (usage.get(right) ?? 0) || left - right)[0] ?? 0;
}

function assignContactIds(mapping: Mapping, existing: Mapping[]) {
  if ("contactId" in mapping) {
    mapping.contactId = nextContactId(existing);
  } else if ("pointer_id" in mapping) {
    mapping.pointer_id = nextContactId(existing);
    if (mapping.type === "Fps" && mapping.touch_mode.type === "dual") {
      mapping.touch_mode.another_pointer_id = nextContactId(existing, [mapping.pointer_id]);
    }
  }
  return mapping;
}

function offsetPosition(position: Position): Position {
  return {
    x: Math.min(1, position.x + 0.025),
    y: Math.min(1, position.y + 0.025),
  };
}

function cloneButtons(binding: ButtonBinding): ButtonBinding {
  return [...binding];
}

function primaryBinding(mapping: Mapping): ButtonBinding | null {
  if (mapping.type === "touch") return mapping.key ? [mapping.key] : [];
  return "bind" in mapping && Array.isArray(mapping.bind) ? cloneButtons(mapping.bind) : null;
}

function directionBinding(mapping: Mapping): DirectionBinding | null {
  if (mapping.type === "dpad") {
    return {
      type: "Button",
      up: mapping.keys.up ? [mapping.keys.up] : [],
      down: mapping.keys.down ? [mapping.keys.down] : [],
      left: mapping.keys.left ? [mapping.keys.left] : [],
      right: mapping.keys.right ? [mapping.keys.right] : [],
    };
  }
  const binding = mapping.type === "DirectionPad" ? mapping.bind : mapping.type === "PadCastSpell" ? mapping.pad_bind : null;
  return binding ? cloneValue(binding) : null;
}

/**
 * Change a controller discriminator by rebuilding the target shape. Only fields
 * with the same runtime meaning cross the boundary; target-specific data starts
 * from createMapping defaults so stale imported fields cannot survive a switch.
 */
export function convertEditorMappingType(
  source: Mapping,
  type: KeyMappingType,
  frame: { width: number; height: number },
): KeyMapping {
  if (source.type === type) return cloneValue(source) as KeyMapping;

  const next = createMapping(type, mappingPosition(source), frame);
  next.id = source.id;
  next.note = "label" in source ? source.label : source.note;

  const sourcePointerId = "contactId" in source
    ? source.contactId
    : "pointer_id" in source
      ? source.pointer_id
      : null;
  if (sourcePointerId !== null && "pointer_id" in next) next.pointer_id = sourcePointerId;

  const binding = primaryBinding(source);
  if (binding && "bind" in next && Array.isArray(next.bind)) next.bind = binding;

  const directions = directionBinding(source);
  if (directions && next.type === "DirectionPad") next.bind = directions;
  if (directions && next.type === "PadCastSpell") next.pad_bind = directions;

  if ("script_hooks" in source && "script_hooks" in next) next.script_hooks = cloneValue(source.script_hooks);
  if ("random_offset_x" in source && "random_offset_x" in next) {
    next.random_offset_x = source.random_offset_x;
    next.random_offset_y = source.random_offset_y;
  }
  if ("duration" in source && "duration" in next) next.duration = source.duration;
  if ("sensitivity_x" in source && "sensitivity_x" in next) {
    next.sensitivity_x = source.sensitivity_x;
    next.sensitivity_y = source.sensitivity_y;
  }
  if ("drag_radius" in source && "drag_radius" in next) next.drag_radius = source.drag_radius;
  if (
    "release_mode" in source
    && "release_mode" in next
    && source.release_mode !== "OnPress"
  ) next.release_mode = source.release_mode;

  return next;
}

export function createEditorMapping(type: KeyMappingType, position: Position, frame: { width: number; height: number }, existing: Mapping[]) {
  return assignContactIds(createMapping(type, position, frame), existing);
}

export function duplicateEditorMapping(source: Mapping, existing: Mapping[]) {
  const duplicate = cloneValue(source);
  duplicate.id = crypto.randomUUID();
  const position = offsetPosition(mappingPosition(source));
  if ("position" in duplicate) duplicate.position = position;
  else {
    duplicate.x = position.x;
    duplicate.y = position.y;
  }
  return assignContactIds(duplicate, existing);
}
