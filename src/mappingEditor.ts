import { createMapping, mappingContactIds, mappingPosition, type Mapping, type Position, type ScrcpyMappingType } from "./types";

const CONTACT_IDS = [0, 1, 2, 3, 4] as const;

function cloneMapping(mapping: Mapping): Mapping {
  return JSON.parse(JSON.stringify(mapping)) as Mapping;
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

export function createEditorMapping(type: ScrcpyMappingType, position: Position, frame: { width: number; height: number }, existing: Mapping[]) {
  return assignContactIds(createMapping(type, position, frame), existing);
}

export function duplicateEditorMapping(source: Mapping, existing: Mapping[]) {
  const duplicate = cloneMapping(source);
  duplicate.id = crypto.randomUUID();
  const position = offsetPosition(mappingPosition(source));
  if ("position" in duplicate) duplicate.position = position;
  else {
    duplicate.x = position.x;
    duplicate.y = position.y;
  }
  return assignContactIds(duplicate, existing);
}
