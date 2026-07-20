export type Orientation =
  | "portrait"
  | "portrait_upside_down"
  | "landscape_left"
  | "landscape_right";

export type Device = {
  udid: string;
  name: string;
  connection: string;
};

export type DeviceStatus = {
  status: string;
  active_udid: string | null;
  error: string | null;
  orientation: Orientation;
  devices: Device[];
};

export type StreamMetrics = {
  decoded_fps: number;
  sent_fps: number;
  jpeg_encode_ms: number;
  megabits_per_second: number;
};

type MappingBase = {
  id: string;
  label: string;
  contactId: number;
  x: number;
  y: number;
};

export type TouchMapping = MappingBase & {
  type: "touch";
  key: string;
};

export type DpadMapping = MappingBase & {
  type: "dpad";
  radius: number;
  keys: { up: string; down: string; left: string; right: string };
};

export type Mapping = TouchMapping | DpadMapping;

export const hardwareButtons = [
  { name: "home" },
  { name: "lock" },
  { name: "volume-up" },
  { name: "volume-down" },
  { name: "mute" },
  { name: "siri" },
  { name: "action" },
] as const;

export type HardwareButtonName = (typeof hardwareButtons)[number]["name"];
export type HardwareBindings = Record<HardwareButtonName, string>;

export const defaultHardwareBindings: HardwareBindings = {
  home: "",
  lock: "",
  "volume-up": "",
  "volume-down": "",
  mute: "",
  siri: "",
  action: "",
};

export type Profile = {
  version: 1;
  name: string;
  mappings: Mapping[];
  hardwareBindings: HardwareBindings;
};

export const defaultProfile: Profile = {
  version: 1,
  name: "default",
  hardwareBindings: { ...defaultHardwareBindings },
  mappings: [
    {
      id: "move",
      type: "dpad",
      label: "Move",
      contactId: 0,
      x: 0.23,
      y: 0.73,
      radius: 0.1,
      keys: { up: "KeyW", down: "KeyS", left: "KeyA", right: "KeyD" },
    },
    { id: "skill-1", type: "touch", label: "Skill 1", contactId: 1, x: 0.78, y: 0.72, key: "Space" },
    { id: "skill-2", type: "touch", label: "Skill 2", contactId: 2, x: 0.87, y: 0.59, key: "KeyJ" },
    { id: "skill-3", type: "touch", label: "Skill 3", contactId: 3, x: 0.72, y: 0.53, key: "KeyK" }
  ]
};
