import { direction, type TouchContact } from "./control";
import type {
  CancelCastMapping,
  FireMapping,
  FpsMapping,
  Mapping,
  MouseCastSpellMapping,
  ObservationMapping,
  PadCastSpellMapping,
  Position,
} from "./types";

export type AdvancedMapping = MouseCastSpellMapping | PadCastSpellMapping | CancelCastMapping | ObservationMapping | FpsMapping | FireMapping;

type PointerState = { mappingId: string; position: Position };
type CastState = PointerState & {
  mapping: MouseCastSpellMapping | PadCastSpellMapping;
  autoReleaseAt: number | null;
};
type CancelState = {
  mappingId: string;
  identity: number;
  start: Position;
  end: Position;
  startedAt: number;
  duration: number;
};
type PendingFpsTouch = {
  kind: "single" | "dual";
  readyAt: number;
  oldIdentity?: number;
  oldPosition?: Position;
  deferredX: number;
  deferredY: number;
};
type FpsState = PointerState & {
  mapping: FpsMapping;
  identity: number;
  touching: boolean;
  pending: PendingFpsTouch | null;
};

export type AdvancedMappingFrame = {
  contacts: TouchContact[];
  activeMappingIds: Set<string>;
  blockDirectionPad: boolean;
};

export type AdvancedKeyResult = { handled: boolean; capturePointer: boolean };

const clamp = (value: number) => Math.max(0, Math.min(1, value));
const bound = (held: ReadonlySet<string>, keys: readonly string[]) => keys.length > 0 && keys.every((key) => held.has(key));
const advancedTypes = new Set<Mapping["type"]>(["MouseCastSpell", "PadCastSpell", "CancelCast", "Observation", "Fps", "Fire"]);

export function isAdvancedMapping(mapping: Mapping): mapping is AdvancedMapping {
  return advancedTypes.has(mapping.type);
}

function clampRadius(origin: Position, position: Position, radius: number, frame: { width: number; height: number }): Position {
  const dx = (position.x - origin.x) * frame.width;
  const dy = (position.y - origin.y) * frame.height;
  const distance = Math.hypot(dx, dy);
  if (radius <= 0 || distance <= radius) return { x: clamp(position.x), y: clamp(position.y) };
  const scale = radius / distance;
  return { x: clamp(origin.x + dx * scale / frame.width), y: clamp(origin.y + dy * scale / frame.height) };
}

function lerp(start: Position, end: Position, amount: number): Position {
  return { x: start.x + (end.x - start.x) * amount, y: start.y + (end.y - start.y) * amount };
}

export class AdvancedMappingRuntime {
  private mappings: AdvancedMapping[] = [];
  private frameSize = { width: 1, height: 1 };
  private observations = new Map<string, PointerState>();
  private fires = new Map<string, PointerState>();
  private fps: FpsState | null = null;
  private cast: CastState | null = null;
  private cancel: CancelState | null = null;

  configure(mappings: readonly Mapping[], frameSize: { width: number; height: number }) {
    this.mappings = mappings.filter(isAdvancedMapping);
    this.frameSize = { width: Math.max(1, frameSize.width), height: Math.max(1, frameSize.height) };
    const ids = new Set(this.mappings.map((mapping) => mapping.id));
    for (const id of this.observations.keys()) if (!ids.has(id)) this.observations.delete(id);
    for (const id of this.fires.keys()) if (!ids.has(id)) this.fires.delete(id);
    if (this.fps) {
      const mapping = this.mapping(this.fps.mappingId, "Fps");
      if (mapping) this.fps.mapping = mapping;
      else this.fps = null;
    }
    if (this.cast) {
      const type = this.cast.mapping.type;
      const mapping = type === "MouseCastSpell"
        ? this.mapping(this.cast.mappingId, "MouseCastSpell")
        : this.mapping(this.cast.mappingId, "PadCastSpell");
      if (mapping) this.cast.mapping = mapping;
      else this.cast = null;
    }
    if (this.cancel && !ids.has(this.cancel.mappingId)) this.cancel = null;
  }

  keyDown(code: string, held: ReadonlySet<string>, now: number): AdvancedKeyResult {
    let handled = false;
    let capturePointer = false;
    for (const mapping of this.mappings) {
      if (!("bind" in mapping) || !mapping.bind.includes(code) || !bound(held, mapping.bind)) continue;
      handled = true;
      switch (mapping.type) {
        case "Observation":
          this.observations.set(mapping.id, { mappingId: mapping.id, position: { ...mapping.position } });
          capturePointer = true;
          break;
        case "Fps":
          if (this.fps?.mappingId === mapping.id) {
            this.fps = null;
          } else {
            this.fps = {
              mappingId: mapping.id,
              mapping,
              identity: mapping.pointer_id,
              position: { ...mapping.position },
              touching: true,
              pending: null,
            };
            capturePointer = true;
          }
          break;
        case "Fire":
          this.fires.set(mapping.id, { mappingId: mapping.id, position: { ...mapping.position } });
          if (!mapping.preserve_fps_control) {
            capturePointer = true;
            if (this.fps) {
              this.fps.position = { ...this.fps.mapping.position };
              this.fps.pending = null;
              this.fps.touching = true;
            }
          }
          break;
        case "MouseCastSpell":
        case "PadCastSpell": {
          if (this.cast?.mappingId === mapping.id && mapping.release_mode === "OnSecondPress") {
            this.cast = null;
            break;
          }
          this.cast = {
            mappingId: mapping.id,
            mapping,
            position: { ...mapping.position },
            autoReleaseAt: mapping.type === "MouseCastSpell" && mapping.release_mode === "OnPress"
              ? now + Math.max(16, mapping.initial_duration)
              : null,
          };
          capturePointer = mapping.type === "MouseCastSpell" && !mapping.cast_no_direction && mapping.release_mode !== "OnPress";
          break;
        }
        case "CancelCast":
          if (this.cast) {
            this.cancel = {
              mappingId: mapping.id,
              identity: this.cast.mapping.pointer_id,
              start: { ...this.cast.position },
              end: { ...mapping.position },
              startedAt: now,
              duration: 150,
            };
            this.cast = null;
          }
          break;
      }
    }
    return { handled, capturePointer };
  }

  keyUp(code: string) {
    for (const mapping of this.mappings) {
      if (!("bind" in mapping) || !mapping.bind.includes(code)) continue;
      if (mapping.type === "Observation") this.observations.delete(mapping.id);
      if (mapping.type === "Fire") {
        this.fires.delete(mapping.id);
        if (!mapping.preserve_fps_control && this.fps) {
          this.fps.position = { ...this.fps.mapping.position };
          this.fps.identity = this.fps.mapping.pointer_id;
          this.fps.touching = true;
          this.fps.pending = null;
        }
      }
      if ((mapping.type === "MouseCastSpell" || mapping.type === "PadCastSpell")
        && mapping.release_mode === "OnRelease" && this.cast?.mappingId === mapping.id) this.cast = null;
    }
  }

  pointerDelta(deltaX: number, deltaY: number, now: number) {
    for (const [id, state] of this.observations) {
      const mapping = this.mapping(id, "Observation");
      if (!mapping) continue;
      state.position = clampRadius(mapping.position, {
        x: state.position.x + deltaX * mapping.sensitivity_x / this.frameSize.width,
        y: state.position.y + deltaY * mapping.sensitivity_y / this.frameSize.height,
      }, mapping.max_radius, this.frameSize);
    }

    if (this.cast?.mapping.type === "MouseCastSpell" && !this.cast.mapping.cast_no_direction) {
      const mapping = this.cast.mapping;
      this.cast.position = clampRadius(mapping.position, {
        x: this.cast.position.x + deltaX * mapping.horizontal_scale_factor / this.frameSize.width,
        y: this.cast.position.y + deltaY * mapping.vertical_scale_factor / this.frameSize.height,
      }, mapping.drag_radius, this.frameSize);
    }

    const interruptingFire = [...this.fires.keys()].some((id) => this.mapping(id, "Fire")?.preserve_fps_control === false);
    for (const [id, state] of this.fires) {
      const mapping = this.mapping(id, "Fire");
      if (!mapping || mapping.preserve_fps_control) continue;
      state.position = {
        x: clamp(state.position.x + deltaX * mapping.sensitivity_x / this.frameSize.width),
        y: clamp(state.position.y + deltaY * mapping.sensitivity_y / this.frameSize.height),
      };
    }
    if (this.fps && !interruptingFire) this.moveFps(deltaX, deltaY, now);
  }

  tick(now: number) {
    const autoReleaseAt = this.cast?.autoReleaseAt;
    if (autoReleaseAt != null && autoReleaseAt <= now) this.cast = null;
    if (this.cancel && now >= this.cancel.startedAt + this.cancel.duration) this.cancel = null;
    const pending = this.fps?.pending;
    if (this.fps && pending && now >= pending.readyAt) {
      this.fps.pending = null;
      this.fps.touching = true;
      this.moveFps(pending.deferredX, pending.deferredY, now);
    }
  }

  frame(held: ReadonlySet<string>, now: number): AdvancedMappingFrame {
    this.tick(now);
    const contacts: TouchContact[] = [];
    const activeMappingIds = new Set<string>();
    for (const [id, state] of this.observations) {
      const mapping = this.mapping(id, "Observation");
      if (mapping && bound(held, mapping.bind)) this.add(contacts, activeMappingIds, id, mapping.pointer_id, state.position);
    }

    const interruptingFire = [...this.fires.keys()].some((id) => this.mapping(id, "Fire")?.preserve_fps_control === false);
    if (this.fps && !interruptingFire) {
      if (this.fps.touching) this.add(contacts, activeMappingIds, this.fps.mappingId, this.fps.identity, this.fps.position);
      const pending = this.fps.pending;
      if (pending?.kind === "dual" && pending.oldIdentity !== undefined && pending.oldPosition) {
        this.add(contacts, activeMappingIds, this.fps.mappingId, pending.oldIdentity, pending.oldPosition);
      }
    }

    for (const [id, state] of this.fires) {
      const mapping = this.mapping(id, "Fire");
      if (mapping && bound(held, mapping.bind)) this.add(contacts, activeMappingIds, id, mapping.pointer_id, state.position);
    }

    if (this.cast) {
      if (this.cast.mapping.type === "PadCastSpell") {
        const vector = direction(this.cast.mapping.pad_bind, held);
        this.cast.position = {
          x: clamp(this.cast.mapping.position.x + vector.dx * this.cast.mapping.drag_radius / this.frameSize.width),
          y: clamp(this.cast.mapping.position.y + vector.dy * this.cast.mapping.drag_radius / this.frameSize.height),
        };
      }
      this.add(contacts, activeMappingIds, this.cast.mappingId, this.cast.mapping.pointer_id, this.cast.position);
    }
    if (this.cancel) {
      const progress = Math.min(1, Math.max(0, (now - this.cancel.startedAt) / this.cancel.duration));
      this.add(contacts, activeMappingIds, this.cancel.mappingId, this.cancel.identity, lerp(this.cancel.start, this.cancel.end, progress));
    }

    return {
      contacts,
      activeMappingIds,
      blockDirectionPad: this.cast?.mapping.type === "PadCastSpell" && this.cast.mapping.block_direction_pad,
    };
  }

  reset() {
    this.observations.clear();
    this.fires.clear();
    this.fps = null;
    this.cast = null;
    this.cancel = null;
  }

  needsPointerCapture() {
    return this.fps !== null
      || this.observations.size > 0
      || (this.cast?.mapping.type === "MouseCastSpell" && !this.cast.mapping.cast_no_direction)
      || [...this.fires.keys()].some((id) => this.mapping(id, "Fire")?.preserve_fps_control === false);
  }

  hasTimedWork() {
    return this.fps?.pending != null || this.cast?.autoReleaseAt != null || this.cancel !== null;
  }

  private moveFps(deltaX: number, deltaY: number, now: number) {
    const fps = this.fps;
    if (!fps) return;
    if (fps.pending) {
      fps.pending.deferredX += deltaX;
      fps.pending.deferredY += deltaY;
      return;
    }
    const mapping = fps.mapping;
    const candidate = {
      x: fps.position.x + deltaX * mapping.sensitivity_x / this.frameSize.width,
      y: fps.position.y + deltaY * mapping.sensitivity_y / this.frameSize.height,
    };
    const marginX = 8 / this.frameSize.width;
    const marginY = 8 / this.frameSize.height;
    const minX = mapping.max_offset_x > 0 ? Math.max(marginX, mapping.position.x - mapping.max_offset_x / this.frameSize.width) : marginX;
    const maxX = mapping.max_offset_x > 0 ? Math.min(1 - marginX, mapping.position.x + mapping.max_offset_x / this.frameSize.width) : 1 - marginX;
    const minY = mapping.max_offset_y > 0 ? Math.max(marginY, mapping.position.y - mapping.max_offset_y / this.frameSize.height) : marginY;
    const maxY = mapping.max_offset_y > 0 ? Math.min(1 - marginY, mapping.position.y + mapping.max_offset_y / this.frameSize.height) : 1 - marginY;
    if (candidate.x > minX && candidate.x < maxX && candidate.y > minY && candidate.y < maxY) {
      fps.position = candidate;
      return;
    }

    const deferredX = mapping.sensitivity_x === 0
      ? 0
      : (candidate.x - Math.max(minX, Math.min(maxX, candidate.x))) * this.frameSize.width / mapping.sensitivity_x;
    const deferredY = mapping.sensitivity_y === 0
      ? 0
      : (candidate.y - Math.max(minY, Math.min(maxY, candidate.y))) * this.frameSize.height / mapping.sensitivity_y;
    if (mapping.touch_mode.type === "single") {
      fps.touching = false;
      fps.position = { ...mapping.position };
      fps.pending = { kind: "single", readyAt: now + Math.max(16, mapping.touch_mode.interval), deferredX, deferredY };
      return;
    }
    const oldIdentity = fps.identity;
    const oldPosition = { x: Math.max(minX, Math.min(maxX, candidate.x)), y: Math.max(minY, Math.min(maxY, candidate.y)) };
    fps.identity = fps.identity === mapping.pointer_id ? mapping.touch_mode.another_pointer_id : mapping.pointer_id;
    fps.position = { ...mapping.position };
    const overlap = mapping.touch_mode.strategy === "overlap";
    const interval = mapping.touch_mode.strategy === "delay" ? mapping.touch_mode.interval : 16;
    fps.touching = overlap;
    fps.pending = {
      kind: "dual",
      readyAt: now + Math.max(16, interval),
      oldIdentity: overlap ? oldIdentity : undefined,
      oldPosition: overlap ? oldPosition : undefined,
      deferredX,
      deferredY,
    };
  }

  private mapping<T extends AdvancedMapping["type"]>(id: string, type: T): Extract<AdvancedMapping, { type: T }> | undefined {
    return this.mappings.find((mapping): mapping is Extract<AdvancedMapping, { type: T }> => mapping.id === id && mapping.type === type);
  }

  private add(contacts: TouchContact[], ids: Set<string>, mappingId: string, identity: number, position: Position) {
    if (contacts.some((contact) => contact.identity === identity) || contacts.length >= 5) return;
    contacts.push({ identity, touching: true, x: clamp(position.x), y: clamp(position.y) });
    ids.add(mappingId);
  }
}
