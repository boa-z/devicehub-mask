export type Orientation = "portrait" | "portrait_upside_down" | "landscape_left" | "landscape_right";

export type Device = { id: string; udid: string; name: string; connection: string };
export type LocationStatus = {
  available: boolean;
  active: boolean;
  latitude: number | null;
  longitude: number | null;
  error: string | null;
};
export type DeviceStatus = { status: string; active_udid: string | null; active_device_id: string | null; error: string | null; orientation: Orientation; devices: Device[]; location: LocationStatus };
export type StreamMetrics = {
  source_fps: number;
  decoded_fps: number;
  published_fps: number;
  sent_fps: number;
  backend_dropped_fps: number;
  jpeg_encode_ms: number;
  frame_age_ms: number;
  websocket_send_ms: number;
  presentation_ack_ms: number;
  megabits_per_second: number;
};
export type ClipboardEvent = {
  from_device: boolean;
  kind: "text" | "image";
  preview: string;
};
export type DeviceEvent = {
  sequence: number;
  kind: "app_installed" | "app_uninstalled" | "activation_state_changed" | "disk_usage_changed" | "device_name_changed" | "lock_state_changed";
};
export type ServicePhase = "connecting" | "ready" | "recovering" | "unavailable" | "stopped";
export type ServiceHealth = {
  name: string;
  phase: ServicePhase;
  attempts: number;
  restarts: number;
  last_error: string | null;
  updated_at_ms: number;
};
export type ProcessPerformance = {
  pid: number;
  name: string;
  cpu_percent: number | null;
  memory_bytes: number | null;
};
export type ProcessEnergy = {
  pid: number;
  name: string;
  total_score: number;
  cpu_score: number;
  gpu_score: number;
  networking_score: number;
  display_score: number;
  location_score: number;
  app_state_score: number;
};
export type PerformanceSnapshot = {
  captured_at_ms: number;
  system_cpu_percent: number | null;
  process_count: number | null;
  logical_cpu_count: number | null;
  top_processes: ProcessPerformance[];
  energy_processes: ProcessEnergy[];
  graphics_fps: number | null;
  gpu_allocated_bytes: number | null;
  gpu_in_use_bytes: number | null;
  gpu_driver_bytes: number | null;
  gpu_recovery_count: number | null;
  network_rx_bytes_per_second: number | null;
  network_tx_bytes_per_second: number | null;
  network_recent_connections: number | null;
};
export type NetworkCaptureState = "idle" | "starting" | "capturing" | "completed" | "failed";
export type NetworkCaptureStopReason = "user_requested" | "duration_limit" | "size_limit" | "session_ended" | "stream_ended";
export type NetworkCaptureStatus = {
  state: NetworkCaptureState;
  packet_count: number;
  bytes_written: number;
  elapsed_ms: number;
  duration_seconds: number | null;
  stop_reason: NetworkCaptureStopReason | null;
  error: string | null;
};
export type BluetoothCaptureState = NetworkCaptureState;
export type BluetoothCaptureStopReason = NetworkCaptureStopReason;
export type BluetoothCaptureStatus = {
  state: BluetoothCaptureState;
  packet_count: number;
  bytes_written: number;
  elapsed_ms: number;
  duration_seconds: number | null;
  stop_reason: BluetoothCaptureStopReason | null;
  error: string | null;
};
export type DeviceBackupState = "idle" | "starting" | "backing_up" | "completed" | "cancelled" | "failed";
export type DeviceBackupStatus = {
  state: DeviceBackupState;
  files_received: number;
  bytes_done: number;
  bytes_total: number;
  progress_percent: number | null;
  elapsed_ms: number;
  full: boolean;
  destination_name: string | null;
  error: string | null;
};
export type DeviceFileKind = "file" | "directory" | "other";
export type DeviceFileEntry = {
  name: string;
  path: string;
  kind: DeviceFileKind;
  size_bytes: number;
  modified: string;
};
export type DeviceFileList = {
  path: string;
  entries: DeviceFileEntry[];
  truncated: boolean;
};
export type DeviceConditionProfile = {
  identifier: string;
  description: string;
};
export type DeviceConditionGroup = {
  identifier: string;
  profiles: DeviceConditionProfile[];
};
export type ActiveDeviceCondition = {
  group_identifier: string;
  profile_identifier: string;
  description: string;
};
export type DeviceConditionStatus = {
  available: boolean;
  groups: DeviceConditionGroup[];
  active: ActiveDeviceCondition | null;
  cleanup_pending: boolean;
  error: string | null;
};
export type PerformanceView = {
  sample: PerformanceSnapshot;
  services: ServiceHealth[];
  sampling: boolean;
  network_capture: NetworkCaptureStatus;
  bluetooth_capture: BluetoothCaptureStatus;
  device_conditions: DeviceConditionStatus;
};
export type DeviceDetails = {
  udid: string;
  name: string;
  product_type: string;
  product_version: string;
  build_version: string | null;
  hardware_model: string | null;
  serial_number: string | null;
  ecid: string | null;
  total_disk_capacity: number | null;
  storage: DeviceStorage | null;
  activation_state: DeviceActivationState | null;
  developer_mode_enabled: boolean | null;
  developer_image_mounted: boolean | null;
  battery: DeviceBattery | null;
};
export type CompanionDevice = {
  identifier: string;
  name: string | null;
  product_type: string | null;
  product_version: string | null;
  build_version: string | null;
};
export type DeviceActivationState = "activated" | "unactivated" | "factory_activated" | "soft_activated" | "unknown";
export type DeviceStorage = {
  data_capacity_bytes: number | null;
  data_available_bytes: number | null;
  system_capacity_bytes: number | null;
  system_available_bytes: number | null;
};
export type DeviceBattery = {
  level_percent: number | null;
  is_charging: boolean | null;
  external_connected: boolean | null;
  fully_charged: boolean | null;
  cycle_count: number | null;
  voltage_mv: number | null;
  instant_amperage_ma: number | null;
  design_capacity_mah: number | null;
  full_charge_capacity_mah: number | null;
  health_percent: number | null;
  time_remaining_minutes: number | null;
  adapter_watts: number | null;
  adapter_name: string | null;
};
export type DeviceApp = {
  bundle_id: string;
  name: string;
  version: string | null;
  bundle_version: string | null;
  is_removable: boolean;
  is_first_party: boolean;
  is_developer_app: boolean;
  documents_available: boolean;
  is_running: boolean | null;
};
export type WdaRunnerStatus = {
  phase: "stopped" | "starting" | "running" | "failed";
  managed: boolean;
  runner_bundle_id: string | null;
  last_error: string | null;
};
export type HomeScreenFolderStep = {
  name: string | null;
  page: number;
  position: number;
};
export type HomeScreenAppLocation = {
  bundle_id: string;
  name: string | null;
  container: "dock" | "page";
  page: number | null;
  position: number;
  folders: HomeScreenFolderStep[];
};
export type HomeScreenLayout = {
  apps: HomeScreenAppLocation[];
  page_count: number;
  metrics: HomeScreenIconMetrics | null;
  truncated: boolean;
};
export type HomeScreenIconMetrics = {
  screen_width: number | null;
  screen_height: number | null;
  icon_width: number | null;
  icon_height: number | null;
  columns: number | null;
  rows: number | null;
  dock_max_count: number | null;
  folder_columns: number | null;
  folder_rows: number | null;
  max_pages: number | null;
  folder_max_pages: number | null;
};
export type AppDocumentEntry = {
  name: string;
  path: string;
  kind: "file" | "directory" | "other";
  size_bytes: number;
  modified: string;
};
export type AppDocumentList = {
  path: string;
  entries: AppDocumentEntry[];
  truncated: boolean;
};
export type DeviceCrashReport = {
  path: string;
  name: string;
  size_bytes: number;
  modified: string;
};
export type DeviceCrashReportList = {
  reports: DeviceCrashReport[];
  truncated: boolean;
};
export type DeviceLogEntry = {
  sequence: number;
  received_at_ms: number;
  message: string;
  level: DeviceLogLevel | null;
  process: string | null;
  pid: number | null;
  subsystem: string | null;
  category: string | null;
  filename: string | null;
};
export type DeviceLogLevel = "notice" | "info" | "debug" | "error" | "fault";
export type DeviceLogSource = "unified" | "syslog";
export type DeviceLogsView = {
  entries: DeviceLogEntry[];
  oldest_sequence: number | null;
  latest_sequence: number | null;
  cursor_lagged: boolean;
  has_more: boolean;
  streaming: boolean;
  source: DeviceLogSource | null;
  service: ServiceHealth | null;
};
export type AppOperationKind = "install" | "uninstall";
export type AppOperationState = "idle" | "running" | "succeeded" | "failed" | "cancelled";
export type AppOperation = {
  id: number;
  kind: AppOperationKind | null;
  state: AppOperationState;
  stage: string | null;
  progress: number | null;
  label: string | null;
  error: string | null;
};
export type ProvisioningProfile = {
  name: string;
  uuid: string;
  team_identifiers: string[];
  application_identifier: string | null;
  creation_date: string | null;
  expiration_date: string | null;
  provisioned_devices: number;
  is_expired: boolean;
  get_task_allow: boolean;
  removal_supported: boolean;
  parse_error: string | null;
};

export type Position = { x: number; y: number };
export type ButtonBinding = string[];
export type ScriptHooks = { before_script: string; after_script: string };
export type DirectionBinding =
  | { type: "Button"; up: ButtonBinding; down: ButtonBinding; left: ButtonBinding; right: ButtonBinding }
  | { type: "JoyStick"; x: string; y: string };

type LegacyBase = { id: string; label: string; contactId: number; x: number; y: number };
export type TouchMapping = LegacyBase & { type: "touch"; key: string };
export type DpadMapping = LegacyBase & { type: "dpad"; radius: number; keys: { up: string; down: string; left: string; right: string } };

type ScrcpyBase = { id: string; type: ScrcpyMappingType; note: string; position: Position };
type PointerBase = ScrcpyBase & { bind: ButtonBinding; pointer_id: number };
type RandomPointerBase = PointerBase & { random_offset_x: number; random_offset_y: number; script_hooks: ScriptHooks };

export type SingleTapMapping = RandomPointerBase & { type: "SingleTap"; duration: number; sync: boolean };
export type RepeatTapMapping = RandomPointerBase & { type: "RepeatTap"; duration: number; interval: number };
export type MultipleTapMapping = Omit<RandomPointerBase, "position"> & { type: "MultipleTap"; position: Position; items: { position: Position; duration: number; wait: number }[] };
export type SwipeMapping = PointerBase & { type: "Swipe"; duration: number; enable_randomization: boolean; positions: Position[]; script_hooks: ScriptHooks };
export type DirectionPadMapping = ScrcpyBase & {
  type: "DirectionPad"; bind: DirectionBinding; pointer_id: number; max_offset_x: number; max_offset_y: number;
  enable_randomization: boolean; initial_duration: number; random_distance_max_scale: number; random_distance_min_scale: number;
  random_offset_x: number; random_offset_y: number; jitter_offset_x: number; jitter_offset_y: number;
  script_hooks: ScriptHooks; up_boost_key: ButtonBinding | null; up_boost_scale: number;
};
export type MouseCastSpellMapping = RandomPointerBase & {
  type: "MouseCastSpell"; center: Position; cast_no_direction: boolean; cast_radius: number; drag_radius: number;
  enable_initial_swipe_randomization: boolean; horizontal_scale_factor: number; vertical_scale_factor: number;
  initial_duration: number; release_mode: "OnPress" | "OnRelease" | "OnSecondPress";
};
export type PadCastSpellMapping = RandomPointerBase & {
  type: "PadCastSpell"; block_direction_pad: boolean; drag_radius: number; enable_randomization: boolean;
  pad_bind: DirectionBinding; release_mode: "OnRelease" | "OnSecondPress";
};
export type CancelCastMapping = ScrcpyBase & { type: "CancelCast"; bind: ButtonBinding; script_hooks: ScriptHooks };
export type ObservationMapping = RandomPointerBase & { type: "Observation"; max_radius: number; sensitivity_x: number; sensitivity_y: number };
export type FpsTouchMode = { type: "single"; interval: number } | { type: "dual"; another_pointer_id: number; strategy: "delay"; interval: number } | { type: "dual"; another_pointer_id: number; strategy: "overlap" };
export type FpsMapping = PointerBase & { type: "Fps"; sensitivity_x: number; sensitivity_y: number; max_offset_x: number; max_offset_y: number; touch_mode: FpsTouchMode };
export type FireMapping = RandomPointerBase & { type: "Fire"; preserve_fps_control: boolean; sensitivity_x: number; sensitivity_y: number };
export type RawInputMapping = ScrcpyBase & { type: "RawInput"; bind: ButtonBinding };
export type ScriptMapping = ScrcpyBase & { type: "Script"; bind: ButtonBinding; pressed_script: string; released_script: string; held_script: string; interval: number };

export type ScrcpyMappingType = "SingleTap" | "RepeatTap" | "MultipleTap" | "Swipe" | "DirectionPad" | "MouseCastSpell" | "PadCastSpell" | "CancelCast" | "Observation" | "Fps" | "Fire" | "RawInput" | "Script";
export const scrcpyMappingTypes: ScrcpyMappingType[] = ["SingleTap", "RepeatTap", "MultipleTap", "Swipe", "DirectionPad", "MouseCastSpell", "PadCastSpell", "CancelCast", "Observation", "Fps", "Fire", "RawInput", "Script"];
export type ScrcpyMapping = SingleTapMapping | RepeatTapMapping | MultipleTapMapping | SwipeMapping | DirectionPadMapping | MouseCastSpellMapping | PadCastSpellMapping | CancelCastMapping | ObservationMapping | FpsMapping | FireMapping | RawInputMapping | ScriptMapping;
export type Mapping = TouchMapping | DpadMapping | ScrcpyMapping;

const hooks = (): ScriptHooks => ({ before_script: "", after_script: "" });
const buttons = (): DirectionBinding => ({ type: "Button", up: [], down: [], left: [], right: [] });
export function createMapping(type: ScrcpyMappingType, position: Position, frame = { width: 1296, height: 2816 }): ScrcpyMapping {
  const id = crypto.randomUUID();
  const base = { id, type, note: "", position };
  const pointer = { ...base, bind: [] as string[], pointer_id: 0 };
  const random = { ...pointer, random_offset_x: 0, random_offset_y: 0, script_hooks: hooks() };
  const distance = Math.min(frame.width, frame.height);
  switch (type) {
    case "SingleTap": return { ...random, type, duration: 50, sync: false };
    case "RepeatTap": return { ...random, type, duration: 50, interval: 100 };
    case "MultipleTap": return { ...random, type, items: [{ position, duration: 50, wait: 0 }] };
    case "Swipe": return { ...pointer, type, duration: 150, enable_randomization: false, positions: [position, { x: Math.min(1, position.x + 0.15), y: position.y }], script_hooks: hooks() };
    case "DirectionPad": return { ...base, type, bind: buttons(), pointer_id: 0, max_offset_x: distance * 0.1, max_offset_y: distance * 0.1, enable_randomization: false, initial_duration: 0, random_distance_min_scale: 1, random_distance_max_scale: 1, random_offset_x: 0, random_offset_y: 0, jitter_offset_x: 0, jitter_offset_y: 0, script_hooks: hooks(), up_boost_key: null, up_boost_scale: 2 };
    case "MouseCastSpell": return { ...random, type, center: { x: 0.5, y: 0.5 }, cast_no_direction: false, cast_radius: distance * 0.15, drag_radius: distance * 0.1, enable_initial_swipe_randomization: false, horizontal_scale_factor: 7, vertical_scale_factor: 10, initial_duration: 0, release_mode: "OnRelease" };
    case "PadCastSpell": return { ...random, type, block_direction_pad: false, drag_radius: distance * 0.1, enable_randomization: false, pad_bind: buttons(), release_mode: "OnRelease" };
    case "CancelCast": return { ...base, type, bind: [], script_hooks: hooks() };
    case "Observation": return { ...random, type, max_radius: 0, sensitivity_x: 0.8, sensitivity_y: 0.8 };
    case "Fps": return { ...pointer, type, sensitivity_x: 0.8, sensitivity_y: 0.8, max_offset_x: 0, max_offset_y: 0, touch_mode: { type: "single", interval: 0 } };
    case "Fire": return { ...random, type, preserve_fps_control: true, sensitivity_x: 0.8, sensitivity_y: 0.8 };
    case "RawInput": return { ...base, type, bind: [] };
    case "Script": return { ...base, type, bind: [], pressed_script: "", released_script: "", held_script: "", interval: 300 };
  }
}

export function mappingPosition(mapping: Mapping): Position { return "position" in mapping ? mapping.position : { x: mapping.x, y: mapping.y }; }
export function mappingLabel(mapping: Mapping): string { return "label" in mapping ? mapping.label : mapping.note || mapping.type; }
export function mappingContactIds(mapping: Mapping): number[] {
  if ("contactId" in mapping) return [mapping.contactId];
  if (!("pointer_id" in mapping)) return [];
  return mapping.type === "Fps" && mapping.touch_mode.type === "dual" ? [mapping.pointer_id, mapping.touch_mode.another_pointer_id] : [mapping.pointer_id];
}

export const hardwareButtons = [{ name: "home" }, { name: "lock" }, { name: "volume-up" }, { name: "volume-down" }, { name: "mute" }, { name: "siri" }, { name: "action" }] as const;
export type HardwareButtonName = (typeof hardwareButtons)[number]["name"];
export type HardwareBindings = Record<HardwareButtonName, string>;
export const defaultHardwareBindings: HardwareBindings = { home: "", lock: "", "volume-up": "", "volume-down": "", mute: "", siri: "", action: "" };
export type Profile = { version: 1; name: string; mappings: Mapping[]; hardwareBindings: HardwareBindings; bundleIdentifiers: string[] };
export const defaultProfile: Profile = { version: 1, name: "default", hardwareBindings: { ...defaultHardwareBindings }, bundleIdentifiers: [], mappings: [
  { id: "move", type: "dpad", label: "Move", contactId: 0, x: 0.23, y: 0.73, radius: 0.1, keys: { up: "KeyW", down: "KeyS", left: "KeyA", right: "KeyD" } },
  { id: "skill-1", type: "touch", label: "Skill 1", contactId: 1, x: 0.78, y: 0.72, key: "Space" },
  { id: "skill-2", type: "touch", label: "Skill 2", contactId: 2, x: 0.87, y: 0.59, key: "KeyJ" },
  { id: "skill-3", type: "touch", label: "Skill 3", contactId: 3, x: 0.72, y: 0.53, key: "KeyK" },
] };
