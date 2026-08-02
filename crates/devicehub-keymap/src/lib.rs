//! Deterministic key-mapping compilation and playback shared by every host adapter.
//!
//! This crate owns profile compilation, runtime transitions, touch composition,
//! and the bounded scripting language. Host adapters only translate runtime
//! output into transport-specific device commands.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devicehub_core::{HardwareButton, KeyMappingProfile, KeyMappingResolution, hardware_button};
use serde_json::{Map, Value};

mod script;

pub use script::{
    ScheduledScriptAction, ScriptAction, ScriptContext, ScriptError, ScriptPlan, ScriptProgram,
    ScriptState, validate_script,
};

const MAX_KEY_CODES: usize = 32;
const MAX_KEY_CODE_LENGTH: usize = 64;
const MAX_TIMING_MS: f64 = 60_000.0;
const MAX_PATH_POINTS: usize = 32;
const MAX_POINTER_SENSITIVITY: f64 = 100.0;
const MAX_POINTER_DELTA: f32 = 16_384.0;
const MAX_GAMEPAD_AXES: usize = 16;
const GAMEPAD_AXIS_DEADZONE: f32 = 0.05;
const MAX_RANDOM_OFFSET: f64 = 16_384.0;
const MAX_RANDOM_DISTANCE_SCALE: f64 = 100.0;
const RANDOM_JITTER_MIN_MS: u64 = 80;
const RANDOM_JITTER_MAX_MS: u64 = 120;
const RANDOM_SWIPE_BEND: f32 = 0.035;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedTouchContact {
    pub identity: u8,
    pub touching: bool,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeymapFrame {
    pub contacts: Vec<NormalizedTouchContact>,
    pub active_mapping_ids: Vec<String>,
    pub matched_mapping_ids: Vec<String>,
    pub unavailable_mapping_ids: Vec<String>,
    pub script_actions: Vec<ScriptAction>,
}

#[derive(Debug, Clone)]
pub struct ActiveHardwareButton {
    pub name: String,
    pub button: HardwareButton,
}

/// Pointer movement in pixels of the profile's target display.
///
/// `cursor_x` and `cursor_y` are optional normalized screen coordinates. They
/// let browser clients reproduce scrcpy-mask's absolute mouse-cast projection
/// while keeping relative deltas compatible with MCP callers and older clients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeymapPointerDelta<'a> {
    pub mapping_id: &'a str,
    pub delta_x: f32,
    pub delta_y: f32,
    pub cursor_x: Option<f32>,
    pub cursor_y: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct CompiledKeymap {
    mappings: Vec<Mapping>,
    hardware_bindings: Vec<HardwareBinding>,
    scripts: CompiledScripts,
    frame: Option<KeyMappingResolution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompileOptions {
    pub allow_scripts: bool,
}

#[derive(Debug, Clone)]
struct HardwareBinding {
    name: String,
    key: String,
    button: HardwareButton,
}

#[derive(Debug, Clone)]
enum Mapping {
    Touch {
        id: String,
        identity: u8,
        position: Point,
        key: String,
    },
    Dpad {
        id: String,
        identity: u8,
        position: Point,
        radius: f32,
        binding: DirectionBinding,
    },
    SingleTap {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        duration_ms: f64,
        sync: bool,
        random_offset: Point,
    },
    Press {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        random_offset: Point,
    },
    RepeatTap {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        duration_ms: f64,
        interval_ms: f64,
        random_offset: Point,
    },
    MultipleTap {
        id: String,
        identity: u8,
        bind: Vec<String>,
        items: Vec<TapItem>,
        random_offset: Point,
    },
    Swipe {
        id: String,
        identity: u8,
        bind: Vec<String>,
        positions: Vec<Point>,
        duration_ms: f64,
        enable_randomization: bool,
    },
    DirectionPad {
        id: String,
        identity: u8,
        position: Point,
        binding: DirectionBinding,
        max_offset_x: f32,
        max_offset_y: f32,
        enable_randomization: bool,
        random_distance_min_scale: f32,
        random_distance_max_scale: f32,
        random_offset: Point,
        jitter_offset: Point,
        up_boost: Option<Vec<String>>,
        up_boost_scale: f32,
        initial_duration: Duration,
        frame: KeyMappingResolution,
    },
    PadCastSpell {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        pad_binding: DirectionBinding,
        drag_radius: f32,
        frame: KeyMappingResolution,
        block_direction_pad: bool,
        release_mode: CastReleaseMode,
        random_offset: Point,
        enable_randomization: bool,
    },
    Pointer {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        sensitivity_x: f32,
        sensitivity_y: f32,
        frame: KeyMappingResolution,
        random_offset: Point,
        kind: PointerKind,
    },
    CancelCast {
        id: String,
        bind: Vec<String>,
        position: Point,
    },
    Unsupported {
        id: String,
        mapping_type: String,
        activation: Activation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Point {
    x: f32,
    y: f32,
}

impl Point {
    const ZERO: Self = Self { x: 0.0, y: 0.0 };
}

#[derive(Debug, Clone)]
struct TapItem {
    position: Point,
    duration_ms: f64,
    wait_ms: f64,
}

#[derive(Debug, Clone)]
enum PointerKind {
    MouseCast {
        center: Point,
        cast_radius: f32,
        drag_radius: f32,
        cast_no_direction: bool,
        horizontal_scale_factor: f32,
        vertical_scale_factor: f32,
        initial_duration: Duration,
        enable_initial_swipe_randomization: bool,
        release_mode: CastReleaseMode,
    },
    Observation {
        max_radius: f32,
    },
    Fps {
        max_offset_x: f32,
        max_offset_y: f32,
        touch_mode: FpsTouchMode,
    },
    Fire {
        preserve_fps_control: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CastReleaseMode {
    Press,
    Release,
    SecondPress,
}

#[derive(Debug, Clone)]
enum FpsTouchMode {
    Single {
        interval: Duration,
    },
    Dual {
        another_identity: u8,
        strategy: FpsHandoffStrategy,
    },
}

#[derive(Debug, Clone)]
enum FpsHandoffStrategy {
    Delay(Duration),
    Overlap,
}

#[derive(Debug, Clone)]
pub struct KeymapRuntimeState {
    gamepad_axes: BTreeMap<String, f32>,
    pulse_starts: BTreeMap<String, Instant>,
    random_states: BTreeMap<String, RandomMappingState>,
    random_seed: u64,
    pointer_positions: BTreeMap<String, Point>,
    mapping_contact_ids: BTreeMap<String, u8>,
    fps: Option<FpsRuntime>,
    cast: Option<CastRuntime>,
    cancel: Option<CancelRuntime>,
    scripts: ScriptRuntime,
}

impl Default for KeymapRuntimeState {
    fn default() -> Self {
        Self {
            gamepad_axes: BTreeMap::new(),
            pulse_starts: BTreeMap::new(),
            random_states: BTreeMap::new(),
            random_seed: random_seed(),
            pointer_positions: BTreeMap::new(),
            mapping_contact_ids: BTreeMap::new(),
            fps: None,
            cast: None,
            cancel: None,
            scripts: ScriptRuntime::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RandomMappingState {
    offset: Point,
    path_bend: Point,
    distance_scale: f32,
    jitter: Point,
    jitter_range: Point,
    next_jitter_at: Instant,
    started_at: Instant,
}

#[derive(Debug, Clone, Default)]
struct CompiledScripts {
    mappings: Vec<CompiledScriptMapping>,
    hooks: Vec<CompiledScriptHooks>,
}

#[derive(Debug, Clone)]
struct CompiledScriptMapping {
    id: String,
    bind: Vec<String>,
    position: Point,
    pressed: ScriptProgram,
    held: ScriptProgram,
    released: ScriptProgram,
    interval: Duration,
}

#[derive(Debug, Clone)]
struct CompiledScriptHooks {
    id: String,
    position: Point,
    before: ScriptProgram,
    after: ScriptProgram,
}

#[derive(Debug, Clone, Default)]
struct ScriptRuntime {
    scopes: BTreeMap<String, ScriptScopeRuntime>,
    hook_ready_at: BTreeMap<String, Instant>,
    queued: Vec<QueuedScriptAction>,
    contacts: BTreeMap<u8, ScriptContact>,
    pending_output: Vec<ScriptAction>,
}

#[derive(Debug, Clone, Default)]
struct ScriptScopeRuntime {
    state: ScriptState,
    active: bool,
    busy_until: Option<Instant>,
    next_held_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct QueuedScriptAction {
    scope: String,
    at: Instant,
    action: ScriptAction,
}

#[derive(Debug, Clone)]
struct ScriptContact {
    scope: String,
    point: Point,
}

#[derive(Debug, Clone)]
struct FpsRuntime {
    mapping_id: String,
    identity: u8,
    position: Point,
    touching: bool,
    pending: Option<PendingFpsTouch>,
}

#[derive(Debug, Clone)]
struct PendingFpsTouch {
    ready_at: Instant,
    old_contact: Option<(u8, Point)>,
    deferred_x: f32,
    deferred_y: f32,
}

#[derive(Debug, Clone)]
struct CastRuntime {
    mapping_id: String,
    position: Point,
    origin: Point,
    target: Point,
    cursor: Option<Point>,
    started_at: Instant,
    auto_release_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct CancelRuntime {
    mapping_id: String,
    identity: u8,
    start: Point,
    end: Point,
    started_at: Instant,
    duration: Duration,
}

#[derive(Debug, Clone)]
enum DirectionBinding {
    Button {
        up: Vec<String>,
        down: Vec<String>,
        left: Vec<String>,
        right: Vec<String>,
    },
    JoyStick {
        x: String,
        y: String,
    },
}

#[derive(Debug, Clone)]
enum Activation {
    All(Vec<String>),
    Never,
}

#[derive(Debug, Clone)]
pub enum KeymapError {
    Invalid(String),
    Unsupported { id: String, mapping_type: String },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Unsupported { id, mapping_type } => write!(
                formatter,
                "mapping {id} uses {mapping_type}, which this keymap session does not support"
            ),
        }
    }
}

impl std::error::Error for KeymapError {}

impl CompiledKeymap {
    pub fn from_profile(
        profile: &KeyMappingProfile,
        fallback_frame: Option<KeyMappingResolution>,
    ) -> Result<Self, KeymapError> {
        Self::from_profile_with_options(profile, fallback_frame, CompileOptions::default())
    }

    pub fn from_profile_with_options(
        profile: &KeyMappingProfile,
        fallback_frame: Option<KeyMappingResolution>,
        options: CompileOptions,
    ) -> Result<Self, KeymapError> {
        let frame = profile.target_resolution.or(fallback_frame);
        let mappings = profile
            .mappings
            .iter()
            .filter(|mapping| {
                !options.allow_scripts
                    || mapping.get("type").and_then(Value::as_str) != Some("Script")
            })
            .map(|mapping| compile_mapping(mapping, frame))
            .collect::<Result<Vec<_>, _>>()?;
        let scripts = if options.allow_scripts {
            compile_scripts(profile, frame)?
        } else {
            CompiledScripts::default()
        };
        let hardware_bindings = profile
            .hardware_bindings
            .iter()
            .filter(|(_, key)| !key.is_empty())
            .map(|(name, key)| {
                let button = hardware_button(name).ok_or_else(|| {
                    KeymapError::Invalid(format!("unknown hardware button binding: {name}"))
                })?;
                validate_key_code(key, "hardware binding")?;
                Ok(HardwareBinding {
                    name: name.clone(),
                    key: key.clone(),
                    button,
                })
            })
            .collect::<Result<Vec<_>, KeymapError>>()?;
        Ok(Self {
            mappings,
            hardware_bindings,
            scripts,
            frame,
        })
    }

    #[cfg(test)]
    pub fn frame(
        &self,
        held: &BTreeSet<String>,
        elapsed: Duration,
    ) -> Result<KeymapFrame, KeymapError> {
        self.frame_with_state(
            held,
            &BTreeMap::new(),
            elapsed,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            None,
            &BTreeSet::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_with_state(
        &self,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
        fallback_elapsed: Duration,
        gamepad_axes: &BTreeMap<String, f32>,
        pulse_elapsed: &BTreeMap<String, f64>,
        forced_active_mapping_ids: &BTreeSet<String>,
        pointer_offsets: &BTreeMap<String, (f32, f32)>,
        random_states: &BTreeMap<String, RandomMappingState>,
        now: Option<Instant>,
        mut mapping_contact_ids: Option<&mut BTreeMap<String, u8>>,
        reserved_contact_ids: &BTreeSet<u8>,
    ) -> Result<KeymapFrame, KeymapError> {
        let held = self.effective_held(held);
        let mut candidates = Vec::with_capacity(self.mappings.len());
        let mut matched_mapping_ids = Vec::new();
        let mut matched_ordinary_mapping_ids = BTreeSet::new();
        for mapping in &self.mappings {
            let elapsed = pulse_elapsed.get(mapping.id()).copied().unwrap_or_else(|| {
                mapping
                    .activation_elapsed(&held, held_for)
                    .unwrap_or(fallback_elapsed)
                    .as_secs_f64()
                    * 1000.0
            });
            let evaluated = mapping.evaluate(
                &held,
                elapsed,
                gamepad_axes,
                forced_active_mapping_ids.contains(mapping.id()),
                pointer_offsets,
                random_states,
                now,
            )?;
            if evaluated.matched {
                matched_mapping_ids.push(evaluated.id.clone());
                if !mapping.is_advanced() {
                    matched_ordinary_mapping_ids.insert(evaluated.id.clone());
                }
            }
            candidates.push(evaluated);
        }

        if let Some(leases) = mapping_contact_ids.as_deref_mut() {
            leases.retain(|id, _| matched_ordinary_mapping_ids.contains(id));
        }

        let mut claimed_keys = HashSet::new();
        let mut contacts = Vec::new();
        let mut active_mapping_ids = Vec::new();
        let mut unavailable_mapping_ids = Vec::new();
        let dynamic_leases_enabled = mapping_contact_ids.is_some();
        let mut used_contact_ids = reserved_contact_ids.clone();
        if dynamic_leases_enabled && let Some(leases) = mapping_contact_ids.as_deref() {
            for mapping_id in &matched_ordinary_mapping_ids {
                if let Some(identity) = leases.get(mapping_id) {
                    used_contact_ids.insert(*identity);
                }
            }
        }
        for candidate in candidates {
            let Some(contact) = candidate.contact else {
                continue;
            };
            let Some(mapping) = self.mapping(&candidate.id) else {
                continue;
            };
            if dynamic_leases_enabled && mapping.is_advanced() {
                continue;
            }
            if !contact.touching
                || candidate
                    .claimed_keys
                    .iter()
                    .any(|key| claimed_keys.contains(key))
            {
                continue;
            }
            claimed_keys.extend(candidate.claimed_keys);
            if dynamic_leases_enabled {
                if let Some(identity) = mapping_contact_ids
                    .as_deref()
                    .and_then(|leases| leases.get(&candidate.id).copied())
                    && !reserved_contact_ids.contains(&identity)
                {
                    used_contact_ids.remove(&identity);
                }
                let Some(identity) = mapping_contact_ids.as_deref_mut().and_then(|leases| {
                    lease_contact_id(leases, &candidate.id, contact.identity, &used_contact_ids)
                }) else {
                    unavailable_mapping_ids.push(candidate.id);
                    continue;
                };
                used_contact_ids.insert(identity);
                contacts.push(NormalizedTouchContact {
                    identity,
                    ..contact
                });
                active_mapping_ids.push(candidate.id);
            } else {
                if contacts
                    .iter()
                    .any(|existing: &NormalizedTouchContact| existing.identity == contact.identity)
                    || contacts.len() >= 5
                {
                    continue;
                }
                contacts.push(contact);
                active_mapping_ids.push(candidate.id);
            }
        }

        Ok(KeymapFrame {
            contacts,
            active_mapping_ids,
            matched_mapping_ids,
            unavailable_mapping_ids,
            script_actions: Vec::new(),
        })
    }

    pub fn active_hardware_buttons(&self, held: &BTreeSet<String>) -> Vec<ActiveHardwareButton> {
        self.hardware_bindings
            .iter()
            .filter(|binding| held.contains(&binding.key))
            .map(|binding| ActiveHardwareButton {
                name: binding.name.clone(),
                button: binding.button,
            })
            .collect()
    }

    pub fn has_matching_mapping(&self, held: &BTreeSet<String>) -> bool {
        self.mappings
            .iter()
            .any(|mapping| mapping.matches_input(held))
            || self
                .scripts
                .mappings
                .iter()
                .any(|mapping| bound(held, &mapping.bind))
    }

    pub fn has_pending_script_work(&self, runtime: &KeymapRuntimeState) -> bool {
        !runtime.scripts.queued.is_empty() || !runtime.scripts.pending_output.is_empty()
    }

    fn hook_ready(&self, runtime: &KeymapRuntimeState, mapping_id: &str, now: Instant) -> bool {
        runtime
            .scripts
            .hook_ready_at
            .get(mapping_id)
            .is_none_or(|ready_at| now >= *ready_at)
    }

    fn begin_randomization(
        &self,
        runtime: &mut KeymapRuntimeState,
        mapping: &Mapping,
        now: Instant,
    ) {
        if let Some(state) = random_state_for_mapping(mapping, &mut runtime.random_seed, now) {
            runtime
                .random_states
                .insert(mapping.id().to_string(), state);
        }
    }

    fn effective_held(&self, held: &BTreeSet<String>) -> BTreeSet<String> {
        let cancelled = self.mappings.iter().any(
            |mapping| matches!(mapping, Mapping::CancelCast { bind, .. } if bound(held, bind)),
        );
        if !cancelled {
            return held.clone();
        }
        let mut result = held.clone();
        for mapping in &self.mappings {
            if let Some(bind) = mapping.cast_binding() {
                for key in bind {
                    result.remove(key);
                }
            }
        }
        result
    }

    pub fn update_runtime(
        &self,
        runtime: &mut KeymapRuntimeState,
        previous: &BTreeSet<String>,
        held: &BTreeSet<String>,
        newly_held: &BTreeSet<String>,
        deltas: &[KeymapPointerDelta<'_>],
        now: Instant,
    ) -> Result<(), KeymapError> {
        self.update_runtime_with_gamepad(
            runtime,
            previous,
            held,
            newly_held,
            deltas,
            &BTreeMap::new(),
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_runtime_with_gamepad(
        &self,
        runtime: &mut KeymapRuntimeState,
        previous: &BTreeSet<String>,
        held: &BTreeSet<String>,
        newly_held: &BTreeSet<String>,
        deltas: &[KeymapPointerDelta<'_>],
        gamepad_axes: &BTreeMap<String, f32>,
        now: Instant,
    ) -> Result<(), KeymapError> {
        runtime.gamepad_axes = normalize_gamepad_axes(gamepad_axes)?;
        self.tick_runtime(runtime, now);
        self.update_script_lifecycles(runtime, previous, held, now)?;
        self.tick_script_runtime(runtime, now)?;

        for mapping in &self.mappings {
            let Mapping::DirectionPad { id, binding, .. } = mapping else {
                continue;
            };
            if binding.has_pressed_key(held, &runtime.gamepad_axes) {
                if !runtime.random_states.contains_key(id) {
                    self.begin_randomization(runtime, mapping, now);
                }
            } else {
                runtime.random_states.remove(id);
            }
        }

        for mapping in &self.mappings {
            let Some(bind) = mapping.binding() else {
                continue;
            };
            let activated = !newly_held.is_empty()
                && bind.iter().any(|key| newly_held.contains(key))
                && bound(held, bind);
            if !activated {
                continue;
            }
            self.begin_randomization(runtime, mapping, now);
            if matches!(
                mapping,
                Mapping::SingleTap { sync: false, .. }
                    | Mapping::MultipleTap { .. }
                    | Mapping::Swipe { .. }
            ) {
                runtime
                    .pulse_starts
                    .entry(mapping.id().to_string())
                    .or_insert(now);
            }
            match mapping {
                Mapping::Pointer {
                    id,
                    identity,
                    position,
                    kind,
                    ..
                } => match kind {
                    PointerKind::Observation { .. } | PointerKind::Fire { .. } => {
                        runtime.pointer_positions.insert(
                            id.clone(),
                            randomized_point(*position, runtime.random_states.get(id)),
                        );
                    }
                    PointerKind::Fps { .. } => {
                        if runtime
                            .fps
                            .as_ref()
                            .is_some_and(|fps| fps.mapping_id == *id)
                        {
                            runtime.fps = None;
                        } else {
                            runtime.fps = Some(FpsRuntime {
                                mapping_id: id.clone(),
                                identity: *identity,
                                position: *position,
                                touching: true,
                                pending: None,
                            });
                        }
                    }
                    PointerKind::MouseCast {
                        release_mode,
                        initial_duration,
                        ..
                    } => {
                        if *release_mode == CastReleaseMode::SecondPress
                            && runtime
                                .cast
                                .as_ref()
                                .is_some_and(|cast| cast.mapping_id == *id)
                        {
                            runtime.cast = None;
                            runtime.random_states.remove(id);
                        } else {
                            let anchor = randomized_point(*position, runtime.random_states.get(id));
                            runtime.cast = Some(CastRuntime {
                                mapping_id: id.clone(),
                                position: anchor,
                                origin: anchor,
                                target: anchor,
                                cursor: None,
                                started_at: now,
                                auto_release_at: (*release_mode == CastReleaseMode::Press).then(
                                    || now + (*initial_duration).max(Duration::from_millis(16)),
                                ),
                            });
                        }
                    }
                },
                Mapping::PadCastSpell {
                    id,
                    position,
                    release_mode,
                    ..
                } => {
                    if *release_mode == CastReleaseMode::SecondPress
                        && runtime
                            .cast
                            .as_ref()
                            .is_some_and(|cast| cast.mapping_id == *id)
                    {
                        runtime.cast = None;
                        runtime.random_states.remove(id);
                    } else {
                        let anchor = randomized_point(*position, runtime.random_states.get(id));
                        runtime.cast = Some(CastRuntime {
                            mapping_id: id.clone(),
                            position: anchor,
                            origin: anchor,
                            target: anchor,
                            cursor: None,
                            started_at: now,
                            auto_release_at: None,
                        });
                    }
                }
                _ => {}
            }
        }

        for mapping in &self.mappings {
            let Mapping::CancelCast { id, bind, position } = mapping else {
                continue;
            };
            if newly_held.iter().any(|key| bind.contains(key))
                && bound(held, bind)
                && let Some(cast) = runtime.cast.take()
                && let Some(identity) = self.mapping_identity(&cast.mapping_id)
            {
                runtime.random_states.remove(&cast.mapping_id);
                runtime.cancel = Some(CancelRuntime {
                    mapping_id: id.clone(),
                    identity,
                    start: cast.position,
                    end: *position,
                    started_at: now,
                    duration: Duration::from_millis(150),
                });
            }
        }

        for mapping in &self.mappings {
            let Some(bind) = mapping.binding() else {
                continue;
            };
            if bound(previous, bind) && !bound(held, bind) {
                match mapping {
                    Mapping::SingleTap { sync: false, .. }
                    | Mapping::MultipleTap { .. }
                    | Mapping::Swipe { .. } => {}
                    Mapping::SingleTap { id, .. }
                    | Mapping::Press { id, .. }
                    | Mapping::RepeatTap { id, .. } => {
                        runtime.random_states.remove(id);
                    }
                    Mapping::Pointer { id, kind, .. } => match kind {
                        PointerKind::Observation { .. } => {
                            runtime.pointer_positions.remove(id);
                            runtime.random_states.remove(id);
                        }
                        PointerKind::Fire {
                            preserve_fps_control,
                        } => {
                            runtime.pointer_positions.remove(id);
                            runtime.random_states.remove(id);
                            if !preserve_fps_control
                                && let Some(fps) = &mut runtime.fps
                                && let Some(Mapping::Pointer {
                                    identity, position, ..
                                }) = self.mapping(&fps.mapping_id)
                            {
                                fps.identity = *identity;
                                fps.position = *position;
                                fps.touching = true;
                                fps.pending = None;
                            }
                        }
                        PointerKind::MouseCast {
                            release_mode: CastReleaseMode::Release,
                            ..
                        } if runtime
                            .cast
                            .as_ref()
                            .is_some_and(|cast| cast.mapping_id == *id) =>
                        {
                            runtime.cast = None;
                            runtime.random_states.remove(id);
                        }
                        _ => {}
                    },
                    Mapping::PadCastSpell {
                        id,
                        release_mode: CastReleaseMode::Release,
                        ..
                    } if runtime
                        .cast
                        .as_ref()
                        .is_some_and(|cast| cast.mapping_id == *id) =>
                    {
                        runtime.cast = None;
                        runtime.random_states.remove(id);
                    }
                    _ => {}
                }
            }
        }

        for delta in deltas {
            self.apply_runtime_delta(runtime, held, *delta, now)?;
        }
        Ok(())
    }

    pub fn frame_with_runtime(
        &self,
        runtime: &mut KeymapRuntimeState,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
        now: Instant,
    ) -> Result<KeymapFrame, KeymapError> {
        self.frame_with_runtime_and_reserved_contacts(
            runtime,
            held,
            held_for,
            now,
            &BTreeSet::new(),
        )
    }

    pub fn frame_with_runtime_and_reserved_contacts(
        &self,
        runtime: &mut KeymapRuntimeState,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
        now: Instant,
        externally_reserved_contact_ids: &BTreeSet<u8>,
    ) -> Result<KeymapFrame, KeymapError> {
        self.tick_runtime(runtime, now);
        self.schedule_held_scripts(runtime, now)?;
        self.tick_script_runtime(runtime, now)?;
        let pulse_elapsed = runtime
            .pulse_starts
            .iter()
            .filter_map(|(id, started_at)| {
                self.mapping(id).map(|_| {
                    let ready_at = runtime
                        .scripts
                        .hook_ready_at
                        .get(id)
                        .copied()
                        .unwrap_or(*started_at);
                    let started_at = (*started_at).max(ready_at);
                    (
                        id.clone(),
                        now.saturating_duration_since(started_at).as_secs_f64() * 1000.0,
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        let forced_active_mapping_ids = pulse_elapsed.keys().cloned().collect::<BTreeSet<_>>();
        let blocked_direction_pad = runtime
            .cast
            .as_ref()
            .filter(|cast| self.hook_ready(runtime, &cast.mapping_id, now))
            .and_then(|cast| self.mapping(&cast.mapping_id))
            .is_some_and(|mapping| {
                matches!(
                    mapping,
                    Mapping::PadCastSpell {
                        block_direction_pad: true,
                        ..
                    }
                )
            });
        let interrupting_fire = self.mappings.iter().any(|mapping| matches!(mapping,
            Mapping::Pointer { id, bind, kind: PointerKind::Fire { preserve_fps_control: false }, .. }
                if bound(held, bind) && runtime.pointer_positions.contains_key(id)));
        let mut reserved_contact_ids = externally_reserved_contact_ids.clone();
        self.reserve_runtime_contact_ids(
            runtime,
            held,
            now,
            interrupting_fire,
            &mut reserved_contact_ids,
        );
        let base = self.frame_with_state(
            held,
            held_for,
            Duration::ZERO,
            &runtime.gamepad_axes,
            &pulse_elapsed,
            &forced_active_mapping_ids,
            &BTreeMap::new(),
            &runtime.random_states,
            Some(now),
            Some(&mut runtime.mapping_contact_ids),
            &reserved_contact_ids,
        )?;
        let gamepad_axes = runtime.gamepad_axes.clone();
        let mut contacts = Vec::new();
        let mut active_mapping_ids = Vec::new();
        let mut unavailable_mapping_ids = base.unavailable_mapping_ids;
        let mut matched_mapping_ids = base
            .matched_mapping_ids
            .into_iter()
            .filter(|id| {
                self.mapping(id)
                    .is_some_and(|mapping| !mapping.is_advanced())
            })
            .collect::<Vec<_>>();

        for mapping in &self.scripts.mappings {
            if bound(held, &mapping.bind) {
                matched_mapping_ids.push(mapping.id.clone());
                active_mapping_ids.push(mapping.id.clone());
            }
        }

        for (identity, contact) in &runtime.scripts.contacts {
            add_runtime_contact(
                &mut contacts,
                &mut active_mapping_ids,
                &contact.scope,
                *identity,
                contact.point,
            );
            matched_mapping_ids.push(contact.scope.clone());
        }

        for mapping in &self.mappings {
            match mapping {
                Mapping::Pointer {
                    id,
                    identity,
                    position,
                    bind,
                    kind: PointerKind::Observation { .. },
                    ..
                } if bound(held, bind) && self.hook_ready(runtime, id, now) => {
                    let point = runtime
                        .pointer_positions
                        .get(id)
                        .copied()
                        .unwrap_or_else(|| {
                            randomized_point(*position, runtime.random_states.get(id))
                        });
                    add_runtime_contact(
                        &mut contacts,
                        &mut active_mapping_ids,
                        id,
                        *identity,
                        point,
                    );
                    matched_mapping_ids.push(id.clone());
                }
                Mapping::Pointer {
                    id,
                    identity,
                    position,
                    bind,
                    kind: PointerKind::Fire { .. },
                    ..
                } if bound(held, bind) && self.hook_ready(runtime, id, now) => {
                    let point = runtime
                        .pointer_positions
                        .get(id)
                        .copied()
                        .unwrap_or_else(|| {
                            randomized_point(*position, runtime.random_states.get(id))
                        });
                    add_runtime_contact(
                        &mut contacts,
                        &mut active_mapping_ids,
                        id,
                        *identity,
                        point,
                    );
                    matched_mapping_ids.push(id.clone());
                }
                _ => {}
            }
        }
        if let Some(fps) = &runtime.fps
            && self.hook_ready(runtime, &fps.mapping_id, now)
        {
            matched_mapping_ids.push(fps.mapping_id.clone());
            if !interrupting_fire {
                if fps.touching {
                    add_runtime_contact(
                        &mut contacts,
                        &mut active_mapping_ids,
                        &fps.mapping_id,
                        fps.identity,
                        fps.position,
                    );
                }
                if let Some(PendingFpsTouch {
                    old_contact: Some((identity, point)),
                    ..
                }) = &fps.pending
                {
                    add_runtime_contact(
                        &mut contacts,
                        &mut active_mapping_ids,
                        &fps.mapping_id,
                        *identity,
                        *point,
                    );
                }
            }
        }
        if runtime
            .cast
            .as_ref()
            .is_some_and(|cast| self.hook_ready(runtime, &cast.mapping_id, now))
            && let Some(cast) = &mut runtime.cast
            && let Some(mapping) = self.mapping(&cast.mapping_id)
        {
            let (identity, point) = match mapping {
                Mapping::PadCastSpell {
                    identity,
                    position,
                    pad_binding,
                    drag_radius,
                    frame,
                    ..
                } => {
                    let (dx, dy) = normalized_direction(pad_binding.direction(held, &gamepad_axes));
                    let random = runtime.random_states.get(&cast.mapping_id).copied();
                    let anchor = randomized_point(*position, random.as_ref());
                    let scale = random.map_or(1.0, |state| state.distance_scale);
                    let jitter = random.map_or(Point::ZERO, |state| state.jitter);
                    cast.target = Point {
                        x: anchor.x + dx * drag_radius * scale / frame.width as f32,
                        y: anchor.y + dy * drag_radius * scale / frame.height as f32,
                    };
                    cast.position = Point {
                        x: clamp(cast.target.x + jitter.x),
                        y: clamp(cast.target.y + jitter.y),
                    };
                    (*identity, cast.position)
                }
                Mapping::Pointer {
                    identity,
                    kind:
                        PointerKind::MouseCast {
                            center,
                            cast_radius,
                            drag_radius,
                            cast_no_direction,
                            horizontal_scale_factor,
                            vertical_scale_factor,
                            initial_duration,
                            enable_initial_swipe_randomization,
                            ..
                        },
                    frame,
                    ..
                } => {
                    if !*cast_no_direction && let Some(cursor) = cast.cursor {
                        cast.target = mouse_cast_target(
                            cursor,
                            MouseCastProjection {
                                center: *center,
                                cast_position: cast.origin,
                                cast_radius: *cast_radius,
                                drag_radius: *drag_radius,
                                horizontal_scale_factor: *horizontal_scale_factor,
                                vertical_scale_factor: *vertical_scale_factor,
                                frame: *frame,
                            },
                        );
                    }
                    let random = runtime.random_states.get(&cast.mapping_id).copied();
                    let progress = if !initial_duration.is_zero() {
                        (now.saturating_duration_since(cast.started_at).as_secs_f32()
                            / initial_duration.as_secs_f32())
                        .clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let bend = random.map_or(Point::ZERO, |state| state.path_bend);
                    let curve = if *enable_initial_swipe_randomization {
                        (std::f32::consts::PI * progress).sin()
                    } else {
                        0.0
                    };
                    cast.position = Point {
                        x: cast.origin.x
                            + (cast.target.x - cast.origin.x) * progress
                            + bend.x * curve,
                        y: cast.origin.y
                            + (cast.target.y - cast.origin.y) * progress
                            + bend.y * curve,
                    };
                    (*identity, cast.position)
                }
                _ => unreachable!(),
            };
            add_runtime_contact(
                &mut contacts,
                &mut active_mapping_ids,
                &cast.mapping_id,
                identity,
                point,
            );
            matched_mapping_ids.push(cast.mapping_id.clone());
        }
        if let Some(cancel) = &runtime.cancel
            && self.hook_ready(runtime, &cancel.mapping_id, now)
        {
            let progress = now
                .saturating_duration_since(cancel.started_at)
                .as_secs_f32()
                / cancel.duration.as_secs_f32();
            let point = Point {
                x: cancel.start.x + (cancel.end.x - cancel.start.x) * progress.clamp(0.0, 1.0),
                y: cancel.start.y + (cancel.end.y - cancel.start.y) * progress.clamp(0.0, 1.0),
            };
            add_runtime_contact(
                &mut contacts,
                &mut active_mapping_ids,
                &cancel.mapping_id,
                cancel.identity,
                point,
            );
            matched_mapping_ids.push(cancel.mapping_id.clone());
        }

        for (contact, id) in base.contacts.into_iter().zip(base.active_mapping_ids) {
            let Some(mapping) = self.mapping(&id) else {
                continue;
            };
            if mapping.is_advanced()
                || (blocked_direction_pad && matches!(mapping, Mapping::DirectionPad { .. }))
                || !self.hook_ready(runtime, &id, now)
            {
                continue;
            }
            if contacts.len() >= 5
                || contacts
                    .iter()
                    .any(|candidate| candidate.identity == contact.identity)
            {
                push_unique(&mut unavailable_mapping_ids, &id);
                continue;
            }
            contacts.push(contact);
            active_mapping_ids.push(id);
        }
        matched_mapping_ids.sort();
        matched_mapping_ids.dedup();
        Ok(KeymapFrame {
            contacts,
            active_mapping_ids,
            matched_mapping_ids,
            unavailable_mapping_ids,
            script_actions: std::mem::take(&mut runtime.scripts.pending_output),
        })
    }

    fn update_script_lifecycles(
        &self,
        runtime: &mut KeymapRuntimeState,
        previous: &BTreeSet<String>,
        held: &BTreeSet<String>,
        now: Instant,
    ) -> Result<(), KeymapError> {
        for mapping in &self.scripts.mappings {
            let was_active = bound(previous, &mapping.bind);
            let is_active = bound(held, &mapping.bind);
            if !was_active && is_active {
                runtime
                    .scripts
                    .scopes
                    .entry(mapping.id.clone())
                    .or_default()
                    .active = true;
                self.schedule_script_program(
                    runtime,
                    &mapping.id,
                    mapping.position,
                    &mapping.pressed,
                    now,
                )?;
                let scope = runtime
                    .scripts
                    .scopes
                    .entry(mapping.id.clone())
                    .or_default();
                scope.next_held_at =
                    (!mapping.held.is_empty()).then(|| scope.busy_until.unwrap_or(now));
            } else if was_active && !is_active {
                let scope = runtime
                    .scripts
                    .scopes
                    .entry(mapping.id.clone())
                    .or_default();
                scope.active = false;
                scope.next_held_at = None;
                self.schedule_script_program(
                    runtime,
                    &mapping.id,
                    mapping.position,
                    &mapping.released,
                    now,
                )?;
            }
        }

        for hooks in &self.scripts.hooks {
            let Some(mapping) = self.mapping(&hooks.id) else {
                continue;
            };
            let was_active = mapping.matches_input(previous);
            let is_active = mapping.matches_input(held);
            if !was_active && is_active {
                self.schedule_script_program(
                    runtime,
                    &hooks.id,
                    hooks.position,
                    &hooks.before,
                    now,
                )?;
                let ready_at = if hooks.before.is_empty() {
                    now
                } else {
                    runtime
                        .scripts
                        .scopes
                        .get(&hooks.id)
                        .and_then(|scope| scope.busy_until)
                        .unwrap_or(now)
                };
                runtime
                    .scripts
                    .hook_ready_at
                    .insert(hooks.id.clone(), ready_at);
            } else if was_active && !is_active {
                let cancelled_before_activation = runtime
                    .scripts
                    .hook_ready_at
                    .get(&hooks.id)
                    .is_some_and(|ready_at| now < *ready_at);
                runtime.scripts.hook_ready_at.remove(&hooks.id);
                if cancelled_before_activation {
                    runtime.pointer_positions.remove(&hooks.id);
                    if runtime
                        .fps
                        .as_ref()
                        .is_some_and(|state| state.mapping_id == hooks.id)
                    {
                        runtime.fps = None;
                    }
                    if runtime
                        .cast
                        .as_ref()
                        .is_some_and(|state| state.mapping_id == hooks.id)
                    {
                        runtime.cast = None;
                    }
                    if runtime
                        .cancel
                        .as_ref()
                        .is_some_and(|state| state.mapping_id == hooks.id)
                    {
                        runtime.cancel = None;
                    }
                }
                self.schedule_script_program(
                    runtime,
                    &hooks.id,
                    hooks.position,
                    &hooks.after,
                    now,
                )?;
            }
        }
        Ok(())
    }

    fn schedule_held_scripts(
        &self,
        runtime: &mut KeymapRuntimeState,
        now: Instant,
    ) -> Result<(), KeymapError> {
        let due = self
            .scripts
            .mappings
            .iter()
            .filter(|mapping| {
                !mapping.held.is_empty()
                    && runtime
                        .scripts
                        .scopes
                        .get(&mapping.id)
                        .is_some_and(|scope| {
                            scope.active && scope.next_held_at.is_some_and(|at| now >= at)
                        })
            })
            .cloned()
            .collect::<Vec<_>>();
        for mapping in due {
            self.schedule_script_program(
                runtime,
                &mapping.id,
                mapping.position,
                &mapping.held,
                now,
            )?;
            let scope = runtime.scripts.scopes.entry(mapping.id).or_default();
            scope.next_held_at = scope
                .active
                .then(|| scope.busy_until.unwrap_or(now) + mapping.interval);
        }
        Ok(())
    }

    fn schedule_script_program(
        &self,
        runtime: &mut KeymapRuntimeState,
        scope_id: &str,
        position: Point,
        program: &ScriptProgram,
        requested_at: Instant,
    ) -> Result<(), KeymapError> {
        if program.is_empty() {
            return Ok(());
        }
        let frame = self.frame.ok_or_else(|| {
            KeymapError::Invalid(
                "Script needs targetResolution or a connected device screen".into(),
            )
        })?;
        let scope = runtime
            .scripts
            .scopes
            .entry(scope_id.to_string())
            .or_default();
        let start = scope.busy_until.unwrap_or(requested_at).max(requested_at);
        let context = ScriptContext {
            frame,
            cursor_x: (position.x * frame.width as f32).round() as u32,
            cursor_y: (position.y * frame.height as f32).round() as u32,
            raw_input: false,
            fps_mode: runtime.fps.is_some(),
        };
        let plan = program
            .plan(context, &mut scope.state)
            .map_err(|error| KeymapError::Invalid(format!("script {scope_id} failed: {error}")))?;
        scope.busy_until = Some(start + plan.duration);
        runtime
            .scripts
            .queued
            .extend(plan.actions.into_iter().map(|action| QueuedScriptAction {
                scope: scope_id.to_string(),
                at: start + action.at,
                action: action.action,
            }));
        runtime.scripts.queued.sort_by_key(|action| action.at);
        Ok(())
    }

    fn tick_script_runtime(
        &self,
        runtime: &mut KeymapRuntimeState,
        now: Instant,
    ) -> Result<(), KeymapError> {
        let due_count = runtime
            .scripts
            .queued
            .partition_point(|action| action.at <= now);
        let due = runtime
            .scripts
            .queued
            .drain(..due_count)
            .collect::<Vec<_>>();
        for action in due {
            self.apply_script_action(runtime, action.scope, action.action, now)?;
        }
        Ok(())
    }

    fn apply_script_action(
        &self,
        runtime: &mut KeymapRuntimeState,
        scope: String,
        action: ScriptAction,
        now: Instant,
    ) -> Result<(), KeymapError> {
        match action {
            ScriptAction::Touch {
                identity,
                touching,
                x,
                y,
            } => {
                if touching {
                    if let Some(contact) = runtime.scripts.contacts.get_mut(&identity) {
                        if contact.scope != scope {
                            return Err(KeymapError::Invalid(format!(
                                "script contact {identity} is already owned by {}",
                                contact.scope
                            )));
                        }
                        contact.point = Point { x, y };
                    } else {
                        runtime.scripts.contacts.insert(
                            identity,
                            ScriptContact {
                                scope,
                                point: Point { x, y },
                            },
                        );
                    }
                } else if runtime
                    .scripts
                    .contacts
                    .get(&identity)
                    .is_some_and(|contact| contact.scope == scope)
                {
                    runtime.scripts.contacts.remove(&identity);
                }
            }
            ScriptAction::EnterFps { mapping_id } => {
                let Some(Mapping::Pointer {
                    identity,
                    position,
                    kind: PointerKind::Fps { .. },
                    ..
                }) = self.mapping(&mapping_id)
                else {
                    return Err(KeymapError::Invalid(format!(
                        "script enter_fps target is not an FPS mapping: {mapping_id}"
                    )));
                };
                runtime.fps = Some(FpsRuntime {
                    mapping_id,
                    identity: *identity,
                    position: *position,
                    touching: true,
                    pending: None,
                });
            }
            ScriptAction::ExitFps => runtime.fps = None,
            ScriptAction::CancelCast { mapping_id } => {
                let Some(Mapping::CancelCast { position, .. }) = self.mapping(&mapping_id) else {
                    return Err(KeymapError::Invalid(format!(
                        "script cancel_cast target is not a CancelCast mapping: {mapping_id}"
                    )));
                };
                if let Some(cast) = runtime.cast.take()
                    && let Some(identity) = self.mapping_identity(&cast.mapping_id)
                {
                    runtime.random_states.remove(&cast.mapping_id);
                    runtime.cancel = Some(CancelRuntime {
                        mapping_id,
                        identity,
                        start: cast.position,
                        end: *position,
                        started_at: now,
                        duration: Duration::from_millis(150),
                    });
                }
            }
            ScriptAction::ReleaseCast => {
                if let Some(cast) = runtime.cast.take() {
                    runtime.random_states.remove(&cast.mapping_id);
                }
                runtime.cancel = None;
            }
            external => runtime.scripts.pending_output.push(external),
        }
        Ok(())
    }

    fn tick_runtime(&self, runtime: &mut KeymapRuntimeState, now: Instant) {
        let expired_pulses = runtime
            .pulse_starts
            .iter()
            .filter_map(|(id, started_at)| {
                self.mapping(id).and_then(|mapping| {
                    let ready_at = runtime
                        .scripts
                        .hook_ready_at
                        .get(id)
                        .copied()
                        .unwrap_or(*started_at);
                    let started_at = (*started_at).max(ready_at);
                    mapping
                        .pulse_duration()
                        .filter(|duration| now.saturating_duration_since(started_at) >= *duration)
                        .map(|_| id.clone())
                })
            })
            .collect::<Vec<_>>();
        for id in expired_pulses {
            runtime.pulse_starts.remove(&id);
            runtime.random_states.remove(&id);
        }
        let auto_released_cast = runtime.cast.as_ref().and_then(|cast| {
            cast.auto_release_at
                .filter(|at| now >= *at)
                .map(|_| cast.mapping_id.clone())
        });
        if let Some(mapping_id) = auto_released_cast {
            runtime.cast = None;
            runtime.random_states.remove(&mapping_id);
        }
        if runtime.cancel.as_ref().is_some_and(|cancel| {
            now.saturating_duration_since(cancel.started_at) >= cancel.duration
        }) {
            runtime.cancel = None;
        }
        let due_jitter = runtime
            .random_states
            .iter()
            .filter(|(_, state)| now >= state.next_jitter_at)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in due_jitter {
            let range = runtime
                .random_states
                .get(&id)
                .map_or(Point::ZERO, |state| state.jitter_range);
            let jitter = random_point(&mut runtime.random_seed, range);
            let next_jitter_at = now + random_jitter_delay(&mut runtime.random_seed);
            if let Some(state) = runtime.random_states.get_mut(&id) {
                state.jitter = jitter;
                state.next_jitter_at = next_jitter_at;
            }
        }
        let pending = runtime.fps.as_mut().and_then(|fps| fps.pending.take());
        if let Some(pending) = pending {
            if now >= pending.ready_at {
                if let Some(fps) = &mut runtime.fps {
                    fps.touching = true;
                }
                self.move_runtime_fps(runtime, pending.deferred_x, pending.deferred_y, now);
            } else if let Some(fps) = &mut runtime.fps {
                fps.pending = Some(pending);
            }
        }
    }

    fn apply_runtime_delta(
        &self,
        runtime: &mut KeymapRuntimeState,
        held: &BTreeSet<String>,
        delta: KeymapPointerDelta<'_>,
        now: Instant,
    ) -> Result<(), KeymapError> {
        if !delta.delta_x.is_finite()
            || !delta.delta_y.is_finite()
            || delta.delta_x.abs() > MAX_POINTER_DELTA
            || delta.delta_y.abs() > MAX_POINTER_DELTA
        {
            return Err(KeymapError::Invalid(format!(
                "pointer delta for {} must be finite and within +/-{MAX_POINTER_DELTA}",
                delta.mapping_id
            )));
        }
        let mapping = self.mapping(delta.mapping_id).ok_or_else(|| {
            KeymapError::Invalid(format!("unknown pointer mapping: {}", delta.mapping_id))
        })?;
        let Mapping::Pointer {
            id,
            position,
            bind,
            sensitivity_x,
            sensitivity_y,
            frame,
            kind,
            ..
        } = mapping
        else {
            return Err(KeymapError::Invalid(format!(
                "mapping {} does not accept pointer deltas",
                delta.mapping_id
            )));
        };
        match kind {
            PointerKind::Observation { max_radius } => {
                if !bound(held, bind) {
                    return Err(pointer_not_active(id));
                }
                let current = runtime
                    .pointer_positions
                    .get(id)
                    .copied()
                    .unwrap_or(*position);
                runtime.pointer_positions.insert(
                    id.clone(),
                    clamp_radius(
                        *position,
                        Point {
                            x: current.x + delta.delta_x * sensitivity_x / frame.width as f32,
                            y: current.y + delta.delta_y * sensitivity_y / frame.height as f32,
                        },
                        *max_radius,
                        *frame,
                    ),
                );
            }
            PointerKind::Fire {
                preserve_fps_control,
            } => {
                if !bound(held, bind) {
                    return Err(pointer_not_active(id));
                }
                if *preserve_fps_control {
                    return Err(KeymapError::Invalid(format!(
                        "mapping {id} preserves FPS control and remains stationary; send deltas to the active Fps mapping"
                    )));
                }
                let current = runtime
                    .pointer_positions
                    .get(id)
                    .copied()
                    .unwrap_or(*position);
                runtime.pointer_positions.insert(
                    id.clone(),
                    Point {
                        x: clamp(current.x + delta.delta_x * sensitivity_x / frame.width as f32),
                        y: clamp(current.y + delta.delta_y * sensitivity_y / frame.height as f32),
                    },
                );
            }
            PointerKind::Fps { .. } => {
                if runtime.fps.as_ref().is_none_or(|fps| fps.mapping_id != *id) {
                    return Err(pointer_not_active(id));
                }
                self.move_runtime_fps(runtime, delta.delta_x, delta.delta_y, now);
            }
            PointerKind::MouseCast {
                drag_radius,
                cast_no_direction,
                ..
            } => {
                if *cast_no_direction
                    || runtime
                        .cast
                        .as_ref()
                        .is_none_or(|cast| cast.mapping_id != *id)
                {
                    return Err(pointer_not_active(id));
                }
                if let Some(cast) = &mut runtime.cast {
                    match (delta.cursor_x, delta.cursor_y) {
                        (Some(cursor_x), Some(cursor_y)) => {
                            cast.cursor = Some(normalized_cursor(cursor_x, cursor_y)?);
                        }
                        (Some(_), None) | (None, Some(_)) => {
                            return Err(KeymapError::Invalid(
                                "pointer cursor coordinates must include both x and y".into(),
                            ));
                        }
                        (None, None) => {
                            cast.cursor = None;
                            cast.target = clamp_radius(
                                cast.origin,
                                Point {
                                    x: cast.target.x
                                        + delta.delta_x * sensitivity_x / frame.width as f32,
                                    y: cast.target.y
                                        + delta.delta_y * sensitivity_y / frame.height as f32,
                                },
                                *drag_radius,
                                *frame,
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn move_runtime_fps(
        &self,
        runtime: &mut KeymapRuntimeState,
        delta_x: f32,
        delta_y: f32,
        now: Instant,
    ) {
        let Some(fps) = &mut runtime.fps else { return };
        let Some(Mapping::Pointer {
            identity,
            position,
            sensitivity_x,
            sensitivity_y,
            frame,
            kind:
                PointerKind::Fps {
                    max_offset_x,
                    max_offset_y,
                    touch_mode,
                },
            ..
        }) = self.mapping(&fps.mapping_id)
        else {
            return;
        };
        if let Some(pending) = &mut fps.pending {
            pending.deferred_x += delta_x;
            pending.deferred_y += delta_y;
            return;
        }
        let candidate = Point {
            x: fps.position.x + delta_x * sensitivity_x / frame.width as f32,
            y: fps.position.y + delta_y * sensitivity_y / frame.height as f32,
        };
        let margin_x = 8.0 / frame.width as f32;
        let margin_y = 8.0 / frame.height as f32;
        let min_x = if *max_offset_x > 0.0 {
            margin_x.max(position.x - max_offset_x / frame.width as f32)
        } else {
            margin_x
        };
        let max_x = if *max_offset_x > 0.0 {
            (1.0 - margin_x).min(position.x + max_offset_x / frame.width as f32)
        } else {
            1.0 - margin_x
        };
        let min_y = if *max_offset_y > 0.0 {
            margin_y.max(position.y - max_offset_y / frame.height as f32)
        } else {
            margin_y
        };
        let max_y = if *max_offset_y > 0.0 {
            (1.0 - margin_y).min(position.y + max_offset_y / frame.height as f32)
        } else {
            1.0 - margin_y
        };
        if candidate.x > min_x && candidate.x < max_x && candidate.y > min_y && candidate.y < max_y
        {
            fps.position = candidate;
            return;
        }
        let deferred_x = if *sensitivity_x == 0.0 {
            0.0
        } else {
            (candidate.x - candidate.x.clamp(min_x, max_x)) * frame.width as f32 / sensitivity_x
        };
        let deferred_y = if *sensitivity_y == 0.0 {
            0.0
        } else {
            (candidate.y - candidate.y.clamp(min_y, max_y)) * frame.height as f32 / sensitivity_y
        };
        let (ready_at, old_contact, next_identity, touching) = match touch_mode {
            FpsTouchMode::Single { interval } => (
                now + (*interval).max(Duration::from_millis(16)),
                None,
                *identity,
                false,
            ),
            FpsTouchMode::Dual {
                another_identity,
                strategy,
            } => {
                let next = if fps.identity == *identity {
                    *another_identity
                } else {
                    *identity
                };
                let (duration, old_contact, touching) = match strategy {
                    FpsHandoffStrategy::Delay(value) => {
                        ((*value).max(Duration::from_millis(16)), None, false)
                    }
                    FpsHandoffStrategy::Overlap => (
                        Duration::from_millis(16),
                        Some((
                            fps.identity,
                            Point {
                                x: candidate.x.clamp(min_x, max_x),
                                y: candidate.y.clamp(min_y, max_y),
                            },
                        )),
                        true,
                    ),
                };
                (now + duration, old_contact, next, touching)
            }
        };
        fps.identity = next_identity;
        fps.position = *position;
        fps.touching = touching;
        fps.pending = Some(PendingFpsTouch {
            ready_at,
            old_contact,
            deferred_x,
            deferred_y,
        });
    }

    fn mapping(&self, id: &str) -> Option<&Mapping> {
        self.mappings.iter().find(|mapping| mapping.id() == id)
    }

    fn reserve_runtime_contact_ids(
        &self,
        runtime: &KeymapRuntimeState,
        held: &BTreeSet<String>,
        now: Instant,
        interrupting_fire: bool,
        reserved: &mut BTreeSet<u8>,
    ) {
        reserved.extend(runtime.scripts.contacts.keys().copied());

        for mapping in &self.mappings {
            match mapping {
                Mapping::Pointer {
                    id,
                    identity,
                    bind,
                    kind: PointerKind::Observation { .. } | PointerKind::Fire { .. },
                    ..
                } if bound(held, bind) && self.hook_ready(runtime, id, now) => {
                    reserved.insert(*identity);
                }
                _ => {}
            }
        }

        if !interrupting_fire
            && let Some(fps) = &runtime.fps
            && self.hook_ready(runtime, &fps.mapping_id, now)
        {
            reserved.insert(fps.identity);
            if let Some(PendingFpsTouch {
                old_contact: Some((identity, _)),
                ..
            }) = &fps.pending
            {
                reserved.insert(*identity);
            }
        }

        if let Some(cast) = &runtime.cast
            && self.hook_ready(runtime, &cast.mapping_id, now)
            && let Some(identity) = self.mapping_identity(&cast.mapping_id)
        {
            reserved.insert(identity);
        }
        if let Some(cancel) = &runtime.cancel
            && self.hook_ready(runtime, &cancel.mapping_id, now)
        {
            reserved.insert(cancel.identity);
        }
    }

    fn mapping_identity(&self, id: &str) -> Option<u8> {
        self.mapping(id).and_then(Mapping::identity)
    }
}

fn lease_contact_id(
    leases: &mut BTreeMap<String, u8>,
    mapping_id: &str,
    preferred: u8,
    occupied: &BTreeSet<u8>,
) -> Option<u8> {
    if let Some(identity) = leases.get(mapping_id).copied() {
        return (!occupied.contains(&identity)).then_some(identity);
    }

    let identity = std::iter::once(preferred)
        .chain(0..5)
        .find(|identity| !occupied.contains(identity))?;
    leases.insert(mapping_id.to_string(), identity);
    Some(identity)
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn add_runtime_contact(
    contacts: &mut Vec<NormalizedTouchContact>,
    ids: &mut Vec<String>,
    id: &str,
    identity: u8,
    point: Point,
) {
    if contacts.len() >= 5 || contacts.iter().any(|contact| contact.identity == identity) {
        return;
    }
    contacts.push(NormalizedTouchContact {
        identity,
        touching: true,
        x: clamp(point.x),
        y: clamp(point.y),
    });
    if !ids.iter().any(|active| active == id) {
        ids.push(id.into());
    }
}

fn pointer_not_active(id: &str) -> KeymapError {
    KeymapError::Invalid(format!(
        "mapping {id} must be active before applying a pointer delta"
    ))
}

fn random_seed() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let seed = time ^ 0x9e37_79b9_7f4a_7c15;
    if seed == 0 { 1 } else { seed }
}

fn next_random_u64(seed: &mut u64) -> u64 {
    let mut value = *seed;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *seed = if value == 0 { 1 } else { value };
    *seed
}

fn random_unit(seed: &mut u64) -> f32 {
    (next_random_u64(seed) as f64 / u64::MAX as f64) as f32
}

fn random_signed(seed: &mut u64) -> f32 {
    random_unit(seed) * 2.0 - 1.0
}

fn random_point(seed: &mut u64, range: Point) -> Point {
    Point {
        x: random_signed(seed) * range.x,
        y: random_signed(seed) * range.y,
    }
}

fn random_jitter_delay(seed: &mut u64) -> Duration {
    let span = RANDOM_JITTER_MAX_MS - RANDOM_JITTER_MIN_MS + 1;
    Duration::from_millis(RANDOM_JITTER_MIN_MS + (random_unit(seed) * span as f32) as u64)
}

fn randomized_point(position: Point, state: Option<&RandomMappingState>) -> Point {
    state.map_or(position, |state| Point {
        x: position.x + state.offset.x,
        y: position.y + state.offset.y,
    })
}

fn random_state_for_mapping(
    mapping: &Mapping,
    seed: &mut u64,
    now: Instant,
) -> Option<RandomMappingState> {
    let mut offset_range = Point::ZERO;
    let mut random_path = false;
    let mut random_distance = None;
    let mut jitter_range = Point::ZERO;
    let mut needs_lifecycle_state = false;

    match mapping {
        Mapping::SingleTap { random_offset, .. }
        | Mapping::Press { random_offset, .. }
        | Mapping::RepeatTap { random_offset, .. }
        | Mapping::MultipleTap { random_offset, .. } => {
            offset_range = *random_offset;
        }
        Mapping::Swipe {
            enable_randomization,
            ..
        } => random_path = *enable_randomization,
        Mapping::DirectionPad {
            enable_randomization,
            random_distance_min_scale,
            random_distance_max_scale,
            random_offset,
            jitter_offset,
            initial_duration,
            ..
        } => {
            needs_lifecycle_state = !initial_duration.is_zero();
            if *enable_randomization {
                offset_range = *random_offset;
                random_distance = Some((*random_distance_min_scale, *random_distance_max_scale));
                jitter_range = *jitter_offset;
            }
        }
        Mapping::PadCastSpell {
            random_offset,
            enable_randomization,
            drag_radius,
            frame,
            ..
        } => {
            offset_range = *random_offset;
            if *enable_randomization {
                jitter_range = Point {
                    x: drag_radius * 0.1 / frame.width as f32,
                    y: drag_radius * 0.1 / frame.height as f32,
                };
            }
        }
        Mapping::Pointer {
            random_offset,
            kind,
            ..
        } => {
            offset_range = *random_offset;
            if let PointerKind::MouseCast {
                enable_initial_swipe_randomization,
                ..
            } = kind
            {
                random_path = *enable_initial_swipe_randomization;
            }
        }
        Mapping::Touch { .. }
        | Mapping::Dpad { .. }
        | Mapping::CancelCast { .. }
        | Mapping::Unsupported { .. } => {}
    }

    if offset_range == Point::ZERO
        && !random_path
        && random_distance.is_none()
        && jitter_range == Point::ZERO
        && !needs_lifecycle_state
    {
        return None;
    }

    let distance_scale = random_distance.map_or(1.0, |(minimum, maximum)| {
        if (maximum - minimum).abs() < f32::EPSILON {
            minimum
        } else {
            minimum + random_unit(seed) * (maximum - minimum)
        }
    });
    Some(RandomMappingState {
        offset: random_point(seed, offset_range),
        path_bend: if random_path {
            Point {
                x: random_signed(seed) * RANDOM_SWIPE_BEND,
                y: random_signed(seed) * RANDOM_SWIPE_BEND,
            }
        } else {
            Point::ZERO
        },
        distance_scale,
        jitter: Point::ZERO,
        jitter_range,
        next_jitter_at: now + random_jitter_delay(seed),
        started_at: now,
    })
}

fn clamp_radius(origin: Point, point: Point, radius: f32, frame: KeyMappingResolution) -> Point {
    let dx = (point.x - origin.x) * frame.width as f32;
    let dy = (point.y - origin.y) * frame.height as f32;
    let distance = dx.hypot(dy);
    if radius <= 0.0 || distance <= radius {
        return Point {
            x: clamp(point.x),
            y: clamp(point.y),
        };
    }
    let scale = radius / distance;
    Point {
        x: clamp(origin.x + dx * scale / frame.width as f32),
        y: clamp(origin.y + dy * scale / frame.height as f32),
    }
}

#[derive(Debug, Clone, Copy)]
struct MouseCastProjection {
    center: Point,
    cast_position: Point,
    cast_radius: f32,
    drag_radius: f32,
    horizontal_scale_factor: f32,
    vertical_scale_factor: f32,
    frame: KeyMappingResolution,
}

fn mouse_cast_target(cursor: Point, projection: MouseCastProjection) -> Point {
    let MouseCastProjection {
        center,
        cast_position,
        cast_radius,
        drag_radius,
        horizontal_scale_factor,
        vertical_scale_factor,
        frame,
    } = projection;
    let width = frame.width as f32;
    let height = frame.height as f32;
    let mut delta = Point {
        x: (cursor.x - center.x) * width,
        y: (cursor.y - center.y) * height,
    };
    let mut radius = cast_radius.max(0.0) / height;
    let drag = drag_radius.max(0.0) / height;
    let (scale_x, scale_y) = if horizontal_scale_factor > vertical_scale_factor {
        let ratio = vertical_scale_factor / horizontal_scale_factor;
        radius *= ratio;
        (1.0, ratio)
    } else {
        let ratio = horizontal_scale_factor / vertical_scale_factor;
        radius *= ratio;
        (ratio, 1.0)
    };
    delta.x = delta.x / width * scale_x;
    delta.y = delta.y / height * scale_y;
    let distance = delta.x.hypot(delta.y);
    let scaled = if radius <= f32::EPSILON {
        Point::ZERO
    } else if distance > radius {
        Point {
            x: delta.x / distance * drag,
            y: delta.y / distance * drag,
        }
    } else {
        Point {
            x: delta.x / radius * drag,
            y: delta.y / radius * drag,
        }
    };
    Point {
        x: clamp(cast_position.x + scaled.x),
        y: clamp(cast_position.y + scaled.y),
    }
}

impl Mapping {
    fn matches_input(&self, held: &BTreeSet<String>) -> bool {
        match self {
            Self::Touch { key, .. } => held.contains(key),
            Self::Dpad { binding, .. } | Self::DirectionPad { binding, .. } => {
                binding.has_pressed_key(held, &BTreeMap::new())
            }
            Self::Unsupported { activation, .. } => activation.is_active(held),
            _ => self.binding().is_some_and(|bind| bound(held, bind)),
        }
    }

    fn binding(&self) -> Option<&[String]> {
        match self {
            Self::Touch { .. } | Self::Dpad { .. } | Self::DirectionPad { .. } => None,
            Self::SingleTap { bind, .. }
            | Self::Press { bind, .. }
            | Self::RepeatTap { bind, .. }
            | Self::MultipleTap { bind, .. }
            | Self::Swipe { bind, .. }
            | Self::PadCastSpell { bind, .. }
            | Self::Pointer { bind, .. }
            | Self::CancelCast { bind, .. } => Some(bind),
            Self::Unsupported {
                activation: Activation::All(bind),
                ..
            } => Some(bind),
            Self::Unsupported {
                activation: Activation::Never,
                ..
            } => None,
        }
    }

    fn identity(&self) -> Option<u8> {
        match self {
            Self::Touch { identity, .. }
            | Self::Dpad { identity, .. }
            | Self::SingleTap { identity, .. }
            | Self::Press { identity, .. }
            | Self::RepeatTap { identity, .. }
            | Self::MultipleTap { identity, .. }
            | Self::Swipe { identity, .. }
            | Self::DirectionPad { identity, .. }
            | Self::PadCastSpell { identity, .. }
            | Self::Pointer { identity, .. } => Some(*identity),
            Self::CancelCast { .. } | Self::Unsupported { .. } => None,
        }
    }

    fn is_advanced(&self) -> bool {
        matches!(
            self,
            Self::PadCastSpell { .. } | Self::Pointer { .. } | Self::CancelCast { .. }
        )
    }

    fn pulse_duration(&self) -> Option<Duration> {
        let milliseconds = match self {
            Self::SingleTap {
                duration_ms,
                sync: false,
                ..
            }
            | Self::Swipe { duration_ms, .. } => *duration_ms,
            Self::MultipleTap { items, .. } => items
                .iter()
                .map(|item| item.wait_ms + item.duration_ms)
                .sum(),
            _ => return None,
        };
        Some(Duration::from_secs_f64(milliseconds / 1000.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate(
        &self,
        held: &BTreeSet<String>,
        elapsed_ms: f64,
        gamepad_axes: &BTreeMap<String, f32>,
        forced_active: bool,
        pointer_offsets: &BTreeMap<String, (f32, f32)>,
        random_states: &BTreeMap<String, RandomMappingState>,
        now: Option<Instant>,
    ) -> Result<EvaluatedMapping, KeymapError> {
        match self {
            Self::Touch {
                id,
                identity,
                position,
                key,
            } => {
                let matched = held.contains(key);
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    *position,
                    matched,
                    if matched {
                        vec![key.clone()]
                    } else {
                        Vec::new()
                    },
                    matched,
                ))
            }
            Self::Dpad {
                id,
                identity,
                position,
                radius,
                binding,
            } => {
                let (dx, dy) = binding.direction(held, gamepad_axes);
                let matched = binding.has_pressed_key(held, gamepad_axes);
                let touching = dx != 0.0 || dy != 0.0;
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: clamp(position.x + dx * radius),
                        y: clamp(position.y + dy * radius),
                    },
                    touching,
                    if touching {
                        binding.pressed_keys(held, gamepad_axes)
                    } else {
                        Vec::new()
                    },
                    matched,
                ))
            }
            Self::SingleTap {
                id,
                identity,
                position,
                bind,
                duration_ms,
                sync,
                random_offset: _,
            } => {
                let matched = forced_active || bound(held, bind);
                let touching = matched && (*sync || elapsed_ms < *duration_ms);
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    randomized_point(*position, random_states.get(id)),
                    touching,
                    claimed_binding_keys(touching, bind),
                    matched,
                ))
            }
            Self::Press {
                id,
                identity,
                position,
                bind,
                random_offset: _,
            } => {
                let matched = bound(held, bind);
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    randomized_point(*position, random_states.get(id)),
                    matched,
                    claimed_binding_keys(matched, bind),
                    matched,
                ))
            }
            Self::RepeatTap {
                id,
                identity,
                position,
                bind,
                duration_ms,
                interval_ms,
                random_offset: _,
            } => {
                let matched = bound(held, bind);
                let period = (*duration_ms + *interval_ms).max(1.0);
                let touching = matched && elapsed_ms % period < *duration_ms;
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    randomized_point(*position, random_states.get(id)),
                    touching,
                    claimed_binding_keys(touching, bind),
                    matched,
                ))
            }
            Self::MultipleTap {
                id,
                identity,
                bind,
                items,
                random_offset: _,
            } => {
                let matched = forced_active || bound(held, bind);
                let mut cursor = 0.0;
                let mut position = items[0].position;
                let mut touching = false;
                for item in items {
                    cursor += item.wait_ms;
                    if elapsed_ms >= cursor && elapsed_ms < cursor + item.duration_ms {
                        position = item.position;
                        touching = matched;
                        break;
                    }
                    cursor += item.duration_ms;
                }
                position = randomized_point(position, random_states.get(id));
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    position,
                    touching,
                    claimed_binding_keys(touching, bind),
                    matched,
                ))
            }
            Self::Swipe {
                id,
                identity,
                bind,
                positions,
                duration_ms,
                enable_randomization,
            } => {
                let matched = forced_active || bound(held, bind);
                let progress = (elapsed_ms / duration_ms.max(1.0)).min(1.0);
                let segment = progress * (positions.len() - 1) as f64;
                let index = segment.floor() as usize;
                let next = (index + 1).min(positions.len() - 1);
                let amount = (segment - index as f64) as f32;
                let start = positions[index];
                let end = positions[next];
                let mut position = Point {
                    x: start.x + (end.x - start.x) * amount,
                    y: start.y + (end.y - start.y) * amount,
                };
                if *enable_randomization && let Some(state) = random_states.get(id) {
                    let curve = (std::f32::consts::PI * progress as f32).sin();
                    position.x += state.path_bend.x * curve;
                    position.y += state.path_bend.y * curve;
                }
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    position,
                    matched,
                    claimed_binding_keys(matched, bind),
                    matched,
                ))
            }
            Self::DirectionPad {
                id,
                identity,
                position,
                binding,
                max_offset_x,
                max_offset_y,
                enable_randomization,
                random_offset: _,
                initial_duration,
                frame,
                up_boost,
                up_boost_scale,
                ..
            } => {
                let (dx, mut dy) = normalized_direction(binding.direction(held, gamepad_axes));
                if dy < 0.0 && up_boost.as_ref().is_some_and(|keys| bound(held, keys)) {
                    dy *= *up_boost_scale;
                }
                let matched = binding.has_pressed_key(held, gamepad_axes);
                let touching = dx != 0.0 || dy != 0.0;
                let random = random_states.get(id);
                let anchor = if *enable_randomization {
                    randomized_point(*position, random)
                } else {
                    *position
                };
                let scale = if *enable_randomization {
                    random.map_or(1.0, |state| state.distance_scale)
                } else {
                    1.0
                };
                let jitter = if *enable_randomization && touching {
                    random.map_or(Point::ZERO, |state| state.jitter)
                } else {
                    Point::ZERO
                };
                let target = Point {
                    x: clamp(anchor.x + dx * max_offset_x * scale / frame.width as f32 + jitter.x),
                    y: clamp(anchor.y + dy * max_offset_y * scale / frame.height as f32 + jitter.y),
                };
                let progress = if !initial_duration.is_zero() {
                    random
                        .and_then(|state| {
                            now.map(|now| now.saturating_duration_since(state.started_at))
                        })
                        .map_or(1.0, |elapsed| {
                            (elapsed.as_secs_f32() / initial_duration.as_secs_f32()).clamp(0.0, 1.0)
                        })
                } else {
                    1.0
                };
                let point = Point {
                    x: anchor.x + (target.x - anchor.x) * progress,
                    y: anchor.y + (target.y - anchor.y) * progress,
                };
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    point,
                    touching,
                    if touching {
                        binding.pressed_keys(held, gamepad_axes)
                    } else {
                        Vec::new()
                    },
                    matched,
                ))
            }
            Self::PadCastSpell {
                id,
                identity,
                position,
                bind,
                pad_binding,
                drag_radius,
                frame,
                random_offset: _,
                enable_randomization,
                ..
            } => {
                let matched = bound(held, bind);
                let (dx, dy) = normalized_direction(pad_binding.direction(held, gamepad_axes));
                let random = random_states.get(id);
                let anchor = randomized_point(*position, random);
                let scale = if *enable_randomization {
                    random.map_or(1.0, |state| state.distance_scale)
                } else {
                    1.0
                };
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: clamp(anchor.x + dx * drag_radius * scale / frame.width as f32),
                        y: clamp(anchor.y + dy * drag_radius * scale / frame.height as f32),
                    },
                    matched,
                    claimed_binding_keys(matched, bind),
                    matched,
                ))
            }
            Self::Pointer {
                id,
                identity,
                position,
                bind,
                ..
            } => {
                let matched = bound(held, bind);
                let (x, y) = pointer_offsets.get(id).copied().unwrap_or_else(|| {
                    let point = randomized_point(*position, random_states.get(id));
                    (point.x, point.y)
                });
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: clamp(x),
                        y: clamp(y),
                    },
                    matched,
                    claimed_binding_keys(matched, bind),
                    matched,
                ))
            }
            Self::CancelCast { id, bind, .. } => {
                Ok(EvaluatedMapping::empty_with_match(id, bound(held, bind)))
            }
            Self::Unsupported {
                id,
                mapping_type,
                activation,
            } => {
                if activation.is_active(held) {
                    return Err(KeymapError::Unsupported {
                        id: id.clone(),
                        mapping_type: mapping_type.clone(),
                    });
                }
                Ok(EvaluatedMapping::empty(id))
            }
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Touch { id, .. }
            | Self::Dpad { id, .. }
            | Self::SingleTap { id, .. }
            | Self::Press { id, .. }
            | Self::RepeatTap { id, .. }
            | Self::MultipleTap { id, .. }
            | Self::Swipe { id, .. }
            | Self::DirectionPad { id, .. }
            | Self::PadCastSpell { id, .. }
            | Self::Pointer { id, .. }
            | Self::CancelCast { id, .. }
            | Self::Unsupported { id, .. } => id,
        }
    }

    fn activation_elapsed(
        &self,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
    ) -> Option<Duration> {
        let bind = match self {
            Self::SingleTap { bind, .. }
            | Self::RepeatTap { bind, .. }
            | Self::MultipleTap { bind, .. }
            | Self::Swipe { bind, .. } => bind,
            _ => return None,
        };
        if !bound(held, bind) {
            return None;
        }
        bind.iter()
            .map(|key| held_for.get(key).copied())
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min()
    }

    fn cast_binding(&self) -> Option<&[String]> {
        match self {
            Self::PadCastSpell { bind, .. } => Some(bind),
            Self::Pointer {
                bind,
                kind: PointerKind::MouseCast { .. },
                ..
            } => Some(bind),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct EvaluatedMapping {
    id: String,
    contact: Option<NormalizedTouchContact>,
    claimed_keys: Vec<String>,
    matched: bool,
}

impl EvaluatedMapping {
    fn empty(id: &str) -> Self {
        Self {
            id: id.into(),
            contact: None,
            claimed_keys: Vec::new(),
            matched: false,
        }
    }

    fn empty_with_match(id: &str, matched: bool) -> Self {
        Self {
            id: id.into(),
            contact: None,
            claimed_keys: Vec::new(),
            matched,
        }
    }

    fn contact(
        id: &str,
        identity: u8,
        position: Point,
        touching: bool,
        claimed_keys: Vec<String>,
        matched: bool,
    ) -> Self {
        Self {
            id: id.into(),
            contact: Some(NormalizedTouchContact {
                identity,
                touching,
                x: position.x,
                y: position.y,
            }),
            claimed_keys,
            matched,
        }
    }
}

impl DirectionBinding {
    fn direction(
        &self,
        held: &BTreeSet<String>,
        gamepad_axes: &BTreeMap<String, f32>,
    ) -> (f32, f32) {
        match self {
            Self::Button {
                up,
                down,
                left,
                right,
            } => {
                let mut dx = f32::from(bound(held, right)) - f32::from(bound(held, left));
                let mut dy = f32::from(bound(held, down)) - f32::from(bound(held, up));
                if dx != 0.0 && dy != 0.0 {
                    dx /= std::f32::consts::SQRT_2;
                    dy /= std::f32::consts::SQRT_2;
                }
                (dx, dy)
            }
            Self::JoyStick { x, y } => (axis_value(gamepad_axes, x), axis_value(gamepad_axes, y)),
        }
    }

    fn has_pressed_key(
        &self,
        held: &BTreeSet<String>,
        gamepad_axes: &BTreeMap<String, f32>,
    ) -> bool {
        match self {
            Self::JoyStick { x, y } => {
                axis_value(gamepad_axes, x).abs() > 0.05 || axis_value(gamepad_axes, y).abs() > 0.05
            }
            Self::Button { .. } => self
                .pressed_keys(held, gamepad_axes)
                .into_iter()
                .next()
                .is_some(),
        }
    }

    fn pressed_keys(
        &self,
        held: &BTreeSet<String>,
        _gamepad_axes: &BTreeMap<String, f32>,
    ) -> Vec<String> {
        let Self::Button {
            up,
            down,
            left,
            right,
        } = self
        else {
            return Vec::new();
        };
        [up, down, left, right]
            .into_iter()
            .flatten()
            .filter(|key| held.contains(*key))
            .cloned()
            .collect()
    }
}

fn axis_value(axes: &BTreeMap<String, f32>, name: &str) -> f32 {
    let value = axes.get(name).copied().unwrap_or_default().clamp(-1.0, 1.0);
    if value.abs() <= GAMEPAD_AXIS_DEADZONE {
        0.0
    } else {
        value
    }
}

fn normalized_direction((x, y): (f32, f32)) -> (f32, f32) {
    let length = x.hypot(y);
    if length > 1.0 {
        (x / length, y / length)
    } else {
        (x, y)
    }
}

impl Activation {
    fn is_active(&self, held: &BTreeSet<String>) -> bool {
        match self {
            Self::All(keys) => bound(held, keys),
            Self::Never => false,
        }
    }
}

pub fn normalize_held_keys(keys: Vec<String>) -> Result<BTreeSet<String>, KeymapError> {
    if keys.is_empty() {
        return Err(KeymapError::Invalid(format!(
            "keys must contain between one and {MAX_KEY_CODES} browser keyboard codes"
        )));
    }
    normalize_key_state(keys)
}

/// Validate a complete key-state update. An empty state is valid and releases
/// all mapped controls in a persistent game session.
pub fn normalize_key_state(keys: Vec<String>) -> Result<BTreeSet<String>, KeymapError> {
    if keys.len() > MAX_KEY_CODES {
        return Err(KeymapError::Invalid(format!(
            "keys must contain at most {MAX_KEY_CODES} browser keyboard codes"
        )));
    }
    let mut held = BTreeSet::new();
    for key in keys {
        validate_key_code(&key, "key")?;
        if !held.insert(key.clone()) {
            return Err(KeymapError::Invalid(format!("duplicate key: {key}")));
        }
    }
    Ok(held)
}

pub fn normalize_gamepad_axes(
    axes: &BTreeMap<String, f32>,
) -> Result<BTreeMap<String, f32>, KeymapError> {
    if axes.len() > MAX_GAMEPAD_AXES {
        return Err(KeymapError::Invalid(format!(
            "gamepad axes must contain at most {MAX_GAMEPAD_AXES} values"
        )));
    }
    let mut normalized = BTreeMap::new();
    for (name, value) in axes {
        if !valid_gamepad_axis_name(name) {
            return Err(KeymapError::Invalid(format!(
                "unsupported gamepad axis: {name}"
            )));
        }
        if !value.is_finite() || !(-1.0..=1.0).contains(value) {
            return Err(KeymapError::Invalid(format!(
                "gamepad axis {name} must be finite and within -1..1"
            )));
        }
        normalized.insert(name.clone(), *value);
    }
    Ok(normalized)
}

fn valid_gamepad_axis_name(name: &str) -> bool {
    matches!(
        name,
        "LeftStickX" | "LeftStickY" | "RightStickX" | "RightStickY" | "LeftZ" | "RightZ"
    ) || name
        .strip_prefix("Other-")
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|value| value < 32)
}

fn compile_scripts(
    profile: &KeyMappingProfile,
    frame: Option<KeyMappingResolution>,
) -> Result<CompiledScripts, KeymapError> {
    let mut scripts = CompiledScripts::default();
    for value in &profile.mappings {
        let mapping = value
            .as_object()
            .ok_or_else(|| KeymapError::Invalid("mapping must be an object".into()))?;
        let id = string_field(mapping, "id")?;
        let mapping_type = string_field(mapping, "type")?;
        if mapping_type == "Script" {
            let pressed = compile_script_field(mapping, "pressed_script", &id)?;
            let held = compile_script_field(mapping, "held_script", &id)?;
            let released = compile_script_field(mapping, "released_script", &id)?;
            if (!pressed.is_empty() || !held.is_empty() || !released.is_empty()) && frame.is_none()
            {
                return Err(KeymapError::Invalid(
                    "Script needs targetResolution or a connected device screen".into(),
                ));
            }
            let interval = mapping
                .get("interval")
                .and_then(Value::as_u64)
                .filter(|interval| (16..=60_000).contains(interval))
                .ok_or_else(|| {
                    KeymapError::Invalid(
                        "Script interval must be between 16 and 60000 milliseconds".into(),
                    )
                })?;
            scripts.mappings.push(CompiledScriptMapping {
                id,
                bind: binding_field(mapping, "bind")?,
                position: position(mapping, "position")?,
                pressed,
                held,
                released,
                interval: Duration::from_millis(interval),
            });
            continue;
        }

        let Some(hooks) = mapping.get("script_hooks") else {
            continue;
        };
        let hooks = hooks.as_object().ok_or_else(|| {
            KeymapError::Invalid(format!("mapping {id} script_hooks must be an object"))
        })?;
        let before = compile_script_field(hooks, "before_script", &id)?;
        let after = compile_script_field(hooks, "after_script", &id)?;
        if before.is_empty() && after.is_empty() {
            continue;
        }
        if frame.is_none() {
            return Err(KeymapError::Invalid(
                "script hooks need targetResolution or a connected device screen".into(),
            ));
        }
        let position = mapping
            .get("position")
            .map(|_| position(mapping, "position"))
            .unwrap_or_else(|| legacy_position(mapping))?;
        scripts.hooks.push(CompiledScriptHooks {
            id,
            position,
            before,
            after,
        });
    }
    Ok(scripts)
}

/// Validates all Script mappings and script hooks without enabling execution.
///
/// A concrete connected-device frame is not required at persistence boundaries;
/// runtime compilation still requires one before any non-empty script can run.
pub fn validate_profile_scripts(profile: &KeyMappingProfile) -> Result<(), KeymapError> {
    let validation_frame = profile.target_resolution.or(Some(KeyMappingResolution {
        width: 16_384,
        height: 16_384,
    }));
    compile_scripts(profile, validation_frame).map(|_| ())
}

fn compile_script_field(
    mapping: &Map<String, Value>,
    field: &str,
    mapping_id: &str,
) -> Result<ScriptProgram, KeymapError> {
    let source = mapping.get(field).and_then(Value::as_str).ok_or_else(|| {
        KeymapError::Invalid(format!(
            "mapping {mapping_id} field {field} must be a string"
        ))
    })?;
    ScriptProgram::compile(source).map_err(|error| {
        KeymapError::Invalid(format!("mapping {mapping_id} field {field}: {error}"))
    })
}

fn compile_mapping(
    value: &Value,
    frame: Option<KeyMappingResolution>,
) -> Result<Mapping, KeymapError> {
    let mapping = value
        .as_object()
        .ok_or_else(|| KeymapError::Invalid("mapping must be an object".into()))?;
    let id = string_field(mapping, "id")?;
    let mapping_type = string_field(mapping, "type")?;
    match mapping_type.as_str() {
        "touch" => Ok(Mapping::Touch {
            id,
            identity: contact_id(mapping, "contactId")?,
            position: legacy_position(mapping)?,
            key: key_field(mapping, "key")?,
        }),
        "dpad" => Ok(Mapping::Dpad {
            id,
            identity: contact_id(mapping, "contactId")?,
            position: legacy_position(mapping)?,
            radius: finite_number(mapping, "radius", 0.0, 1.0)? as f32,
            binding: DirectionBinding::Button {
                up: vec![key_field_from_object(object_field(mapping, "keys")?, "up")?],
                down: vec![key_field_from_object(
                    object_field(mapping, "keys")?,
                    "down",
                )?],
                left: vec![key_field_from_object(
                    object_field(mapping, "keys")?,
                    "left",
                )?],
                right: vec![key_field_from_object(
                    object_field(mapping, "keys")?,
                    "right",
                )?],
            },
        }),
        "SingleTap" => Ok(Mapping::SingleTap {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
            duration_ms: finite_number(mapping, "duration", 1.0, MAX_TIMING_MS)?,
            sync: mapping
                .get("sync")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            random_offset: random_offset(mapping, frame)?,
        }),
        "Press" => Ok(Mapping::Press {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
            random_offset: random_offset(mapping, frame)?,
        }),
        "RepeatTap" => Ok(Mapping::RepeatTap {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
            duration_ms: finite_number(mapping, "duration", 1.0, MAX_TIMING_MS)?,
            interval_ms: finite_number(mapping, "interval", 0.0, MAX_TIMING_MS)?,
            random_offset: random_offset(mapping, frame)?,
        }),
        "MultipleTap" => Ok(Mapping::MultipleTap {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            bind: binding_field(mapping, "bind")?,
            items: tap_items(mapping)?,
            random_offset: random_offset(mapping, frame)?,
        }),
        "Swipe" => Ok(Mapping::Swipe {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            bind: binding_field(mapping, "bind")?,
            positions: positions(mapping)?,
            duration_ms: finite_number(mapping, "duration", 1.0, MAX_TIMING_MS)?,
            enable_randomization: boolean_field_default(mapping, "enable_randomization", false)?,
        }),
        "DirectionPad" => {
            let target_frame = frame.ok_or_else(|| {
                KeymapError::Invalid(
                    "DirectionPad needs targetResolution or a connected device screen".into(),
                )
            })?;
            let random_distance_min_scale = finite_number_default(
                mapping,
                "random_distance_min_scale",
                1.0,
                0.0,
                MAX_RANDOM_DISTANCE_SCALE,
            )? as f32;
            let random_distance_max_scale = finite_number_default(
                mapping,
                "random_distance_max_scale",
                1.0,
                0.0,
                MAX_RANDOM_DISTANCE_SCALE,
            )? as f32;
            if random_distance_min_scale > random_distance_max_scale {
                return Err(KeymapError::Invalid(
                    "DirectionPad random distance minimum scale must not exceed maximum scale"
                        .into(),
                ));
            }
            Ok(Mapping::DirectionPad {
                id,
                identity: contact_id(mapping, "pointer_id")?,
                position: position(mapping, "position")?,
                binding: direction_binding(mapping, "bind")?,
                max_offset_x: finite_number(mapping, "max_offset_x", 0.0, MAX_TIMING_MS)? as f32,
                max_offset_y: finite_number(mapping, "max_offset_y", 0.0, MAX_TIMING_MS)? as f32,
                enable_randomization: boolean_field_default(
                    mapping,
                    "enable_randomization",
                    false,
                )?,
                random_distance_min_scale,
                random_distance_max_scale,
                random_offset: random_offset(mapping, Some(target_frame))?,
                jitter_offset: random_offset_field(
                    mapping,
                    "jitter_offset_x",
                    "jitter_offset_y",
                    Some(target_frame),
                )?,
                up_boost: optional_binding_field(mapping, "up_boost_key")?,
                up_boost_scale: finite_number_default(
                    mapping,
                    "up_boost_scale",
                    1.4,
                    0.0,
                    MAX_POINTER_SENSITIVITY,
                )? as f32,
                initial_duration: Duration::from_secs_f64(
                    finite_number_default(mapping, "initial_duration", 0.0, 0.0, MAX_TIMING_MS)?
                        / 1000.0,
                ),
                frame: target_frame,
            })
        }
        "PadCastSpell" => {
            let target_frame = frame.ok_or_else(|| {
                KeymapError::Invalid(
                    "PadCastSpell needs targetResolution or a connected device screen".into(),
                )
            })?;
            Ok(Mapping::PadCastSpell {
                id,
                identity: contact_id(mapping, "pointer_id")?,
                position: position(mapping, "position")?,
                bind: binding_field(mapping, "bind")?,
                pad_binding: direction_binding(mapping, "pad_bind")?,
                drag_radius: finite_number(mapping, "drag_radius", 0.0, MAX_TIMING_MS)? as f32,
                block_direction_pad: boolean_field(mapping, "block_direction_pad")?,
                release_mode: cast_release_mode(mapping, false)?,
                random_offset: random_offset(mapping, Some(target_frame))?,
                enable_randomization: boolean_field_default(
                    mapping,
                    "enable_randomization",
                    false,
                )?,
                frame: target_frame,
            })
        }
        "MouseCastSpell" => {
            let position = position(mapping, "position")?;
            let center = mapping
                .get("center")
                .map(|value| {
                    value
                        .as_object()
                        .ok_or_else(|| {
                            KeymapError::Invalid("mapping field center must be an object".into())
                        })
                        .and_then(|value| point(value, "center"))
                })
                .transpose()?
                .unwrap_or(position);
            let kind = PointerKind::MouseCast {
                center,
                cast_radius: finite_number_default(
                    mapping,
                    "cast_radius",
                    0.0,
                    0.0,
                    MAX_POINTER_DELTA as f64,
                )? as f32,
                drag_radius: finite_number(mapping, "drag_radius", 0.0, MAX_TIMING_MS)? as f32,
                cast_no_direction: boolean_field(mapping, "cast_no_direction")?,
                horizontal_scale_factor: finite_number_default(
                    mapping,
                    "horizontal_scale_factor",
                    1.0,
                    0.001,
                    MAX_POINTER_SENSITIVITY,
                )? as f32,
                vertical_scale_factor: finite_number_default(
                    mapping,
                    "vertical_scale_factor",
                    1.0,
                    0.001,
                    MAX_POINTER_SENSITIVITY,
                )? as f32,
                initial_duration: Duration::from_secs_f64(
                    finite_number(mapping, "initial_duration", 0.0, MAX_TIMING_MS)? / 1000.0,
                ),
                enable_initial_swipe_randomization: boolean_field_default(
                    mapping,
                    "enable_initial_swipe_randomization",
                    false,
                )?,
                release_mode: cast_release_mode(mapping, true)?,
            };
            pointer_mapping(
                id,
                mapping,
                frame,
                "MouseCastSpell",
                "horizontal_scale_factor",
                "vertical_scale_factor",
                kind,
            )
        }
        "Observation" => pointer_mapping(
            id,
            mapping,
            frame,
            "Observation",
            "sensitivity_x",
            "sensitivity_y",
            PointerKind::Observation {
                max_radius: finite_number(mapping, "max_radius", 0.0, MAX_TIMING_MS)? as f32,
            },
        ),
        "Fps" => {
            let touch_mode = fps_touch_mode(mapping)?;
            pointer_mapping(
                id,
                mapping,
                frame,
                "Fps",
                "sensitivity_x",
                "sensitivity_y",
                PointerKind::Fps {
                    max_offset_x: finite_number(mapping, "max_offset_x", 0.0, MAX_TIMING_MS)?
                        as f32,
                    max_offset_y: finite_number(mapping, "max_offset_y", 0.0, MAX_TIMING_MS)?
                        as f32,
                    touch_mode,
                },
            )
        }
        "Fire" => pointer_mapping(
            id,
            mapping,
            frame,
            "Fire",
            "sensitivity_x",
            "sensitivity_y",
            PointerKind::Fire {
                preserve_fps_control: boolean_field(mapping, "preserve_fps_control")?,
            },
        ),
        "CancelCast" => Ok(Mapping::CancelCast {
            id,
            bind: binding_field(mapping, "bind")?,
            position: position(mapping, "position")?,
        }),
        unsupported => Ok(Mapping::Unsupported {
            id,
            mapping_type: unsupported.into(),
            activation: unsupported_activation(mapping),
        }),
    }
}

fn pointer_mapping(
    id: String,
    mapping: &Map<String, Value>,
    frame: Option<KeyMappingResolution>,
    mapping_type: &str,
    sensitivity_x: &str,
    sensitivity_y: &str,
    kind: PointerKind,
) -> Result<Mapping, KeymapError> {
    Ok(Mapping::Pointer {
        id,
        identity: contact_id(mapping, "pointer_id")?,
        position: position(mapping, "position")?,
        bind: binding_field(mapping, "bind")?,
        sensitivity_x: finite_number(mapping, sensitivity_x, 0.0, MAX_POINTER_SENSITIVITY)? as f32,
        sensitivity_y: finite_number(mapping, sensitivity_y, 0.0, MAX_POINTER_SENSITIVITY)? as f32,
        random_offset: random_offset(mapping, frame)?,
        frame: frame.ok_or_else(|| {
            KeymapError::Invalid(format!(
                "{mapping_type} needs targetResolution or a connected device screen"
            ))
        })?,
        kind,
    })
}

fn boolean_field(mapping: &Map<String, Value>, name: &str) -> Result<bool, KeymapError> {
    mapping
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| KeymapError::Invalid(format!("mapping field {name} must be a boolean")))
}

fn boolean_field_default(
    mapping: &Map<String, Value>,
    name: &str,
    default: bool,
) -> Result<bool, KeymapError> {
    if mapping.contains_key(name) {
        boolean_field(mapping, name)
    } else {
        Ok(default)
    }
}

fn finite_number_default(
    mapping: &Map<String, Value>,
    name: &str,
    default: f64,
    minimum: f64,
    maximum: f64,
) -> Result<f64, KeymapError> {
    if mapping.contains_key(name) {
        finite_number(mapping, name, minimum, maximum)
    } else {
        Ok(default)
    }
}

fn random_offset(
    mapping: &Map<String, Value>,
    frame: Option<KeyMappingResolution>,
) -> Result<Point, KeymapError> {
    random_offset_field(mapping, "random_offset_x", "random_offset_y", frame)
}

fn random_offset_field(
    mapping: &Map<String, Value>,
    x_name: &str,
    y_name: &str,
    frame: Option<KeyMappingResolution>,
) -> Result<Point, KeymapError> {
    let x = finite_number_default(mapping, x_name, 0.0, 0.0, MAX_RANDOM_OFFSET)?;
    let y = finite_number_default(mapping, y_name, 0.0, 0.0, MAX_RANDOM_OFFSET)?;
    Ok(frame.map_or(Point::ZERO, |frame| Point {
        x: (x / frame.width as f64) as f32,
        y: (y / frame.height as f64) as f32,
    }))
}

fn cast_release_mode(
    mapping: &Map<String, Value>,
    allow_on_press: bool,
) -> Result<CastReleaseMode, KeymapError> {
    match string_field(mapping, "release_mode")?.as_str() {
        "OnPress" if allow_on_press => Ok(CastReleaseMode::Press),
        "OnRelease" => Ok(CastReleaseMode::Release),
        "OnSecondPress" => Ok(CastReleaseMode::SecondPress),
        _ => Err(KeymapError::Invalid(
            "mapping field release_mode is not valid for this cast mapping".into(),
        )),
    }
}

fn fps_touch_mode(mapping: &Map<String, Value>) -> Result<FpsTouchMode, KeymapError> {
    let mode = object_field(mapping, "touch_mode")?;
    match string_field(mode, "type")?.as_str() {
        "single" => Ok(FpsTouchMode::Single {
            interval: Duration::from_secs_f64(
                finite_number(mode, "interval", 0.0, MAX_TIMING_MS)? / 1000.0,
            ),
        }),
        "dual" => {
            let another_identity = contact_id(mode, "another_pointer_id")?;
            let primary_identity = contact_id(mapping, "pointer_id")?;
            if another_identity == primary_identity {
                return Err(KeymapError::Invalid(
                    "FPS dual touch identities must be different".into(),
                ));
            }
            let strategy = match string_field(mode, "strategy")?.as_str() {
                "delay" => FpsHandoffStrategy::Delay(Duration::from_secs_f64(
                    finite_number(mode, "interval", 0.0, MAX_TIMING_MS)? / 1000.0,
                )),
                "overlap" => FpsHandoffStrategy::Overlap,
                _ => {
                    return Err(KeymapError::Invalid(
                        "mapping field touch_mode.strategy must be delay or overlap".into(),
                    ));
                }
            };
            Ok(FpsTouchMode::Dual {
                another_identity,
                strategy,
            })
        }
        _ => Err(KeymapError::Invalid(
            "mapping field touch_mode.type must be single or dual".into(),
        )),
    }
}

fn unsupported_activation(mapping: &Map<String, Value>) -> Activation {
    match binding_field(mapping, "bind") {
        Ok(keys) if !keys.is_empty() => Activation::All(keys),
        _ => Activation::Never,
    }
}

fn string_field(mapping: &Map<String, Value>, name: &str) -> Result<String, KeymapError> {
    let value = mapping
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| KeymapError::Invalid(format!("mapping field {name} must be a string")))?;
    Ok(value.into())
}

fn key_field(mapping: &Map<String, Value>, name: &str) -> Result<String, KeymapError> {
    key_field_from_object(mapping, name)
}

fn key_field_from_object(mapping: &Map<String, Value>, name: &str) -> Result<String, KeymapError> {
    let key = string_field(mapping, name)?;
    validate_key_code(&key, &format!("mapping field {name}"))?;
    Ok(key)
}

fn object_field<'a>(
    mapping: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, KeymapError> {
    mapping
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| KeymapError::Invalid(format!("mapping field {name} must be an object")))
}

fn binding_field(mapping: &Map<String, Value>, name: &str) -> Result<Vec<String>, KeymapError> {
    let values = mapping
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| KeymapError::Invalid(format!("mapping field {name} must be an array")))?;
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let key = value.as_str().ok_or_else(|| {
            KeymapError::Invalid(format!("mapping field {name} must contain strings"))
        })?;
        validate_key_code(key, &format!("mapping field {name}"))?;
        result.push(key.into());
    }
    Ok(result)
}

fn optional_binding_field(
    mapping: &Map<String, Value>,
    name: &str,
) -> Result<Option<Vec<String>>, KeymapError> {
    let Some(value) = mapping.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    binding_field(mapping, name).map(Some)
}

fn direction_binding(
    mapping: &Map<String, Value>,
    name: &str,
) -> Result<DirectionBinding, KeymapError> {
    let binding = object_field(mapping, name)?;
    match string_field(binding, "type")?.as_str() {
        "Button" => Ok(DirectionBinding::Button {
            up: binding_field(binding, "up")?,
            down: binding_field(binding, "down")?,
            left: binding_field(binding, "left")?,
            right: binding_field(binding, "right")?,
        }),
        "JoyStick" => Ok(DirectionBinding::JoyStick {
            x: gamepad_axis_field(binding, "x")?,
            y: gamepad_axis_field(binding, "y")?,
        }),
        _ => Err(KeymapError::Invalid(format!(
            "mapping field {name}.type must be Button or JoyStick"
        ))),
    }
}

fn gamepad_axis_field(mapping: &Map<String, Value>, name: &str) -> Result<String, KeymapError> {
    let value = string_field(mapping, name)?;
    if valid_gamepad_axis_name(&value) {
        Ok(value)
    } else {
        Err(KeymapError::Invalid(format!(
            "mapping gamepad axis {name} must be a standard axis or Other-N"
        )))
    }
}

fn contact_id(mapping: &Map<String, Value>, name: &str) -> Result<u8, KeymapError> {
    let identity = mapping
        .get(name)
        .and_then(Value::as_u64)
        .filter(|identity| *identity < 5)
        .ok_or_else(|| {
            KeymapError::Invalid(format!(
                "mapping field {name} must be an integer from 0 to 4"
            ))
        })?;
    Ok(identity as u8)
}

fn position(mapping: &Map<String, Value>, name: &str) -> Result<Point, KeymapError> {
    point(
        mapping
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                KeymapError::Invalid(format!("mapping field {name} must be an object"))
            })?,
        name,
    )
}

fn legacy_position(mapping: &Map<String, Value>) -> Result<Point, KeymapError> {
    Point::new(
        finite_number(mapping, "x", 0.0, 1.0)?,
        finite_number(mapping, "y", 0.0, 1.0)?,
        "mapping",
    )
}

fn point(mapping: &Map<String, Value>, context: &str) -> Result<Point, KeymapError> {
    Point::new(
        finite_number(mapping, "x", 0.0, 1.0)?,
        finite_number(mapping, "y", 0.0, 1.0)?,
        context,
    )
}

impl Point {
    fn new(x: f64, y: f64, context: &str) -> Result<Self, KeymapError> {
        let x = x as f32;
        let y = y as f32;
        if !x.is_finite() || !y.is_finite() {
            return Err(KeymapError::Invalid(format!(
                "{context} coordinates must be finite"
            )));
        }
        Ok(Self { x, y })
    }
}

fn normalized_cursor(x: f32, y: f32) -> Result<Point, KeymapError> {
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(KeymapError::Invalid(
            "pointer cursor coordinates must be finite and within 0..1".into(),
        ));
    }
    Ok(Point { x, y })
}

fn positions(mapping: &Map<String, Value>) -> Result<Vec<Point>, KeymapError> {
    let values = mapping
        .get("positions")
        .and_then(Value::as_array)
        .filter(|values| (2..=MAX_PATH_POINTS).contains(&values.len()))
        .ok_or_else(|| {
            KeymapError::Invalid(format!(
                "mapping field positions must contain 2 to {MAX_PATH_POINTS} points"
            ))
        })?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            point(
                value.as_object().ok_or_else(|| {
                    KeymapError::Invalid(format!("mapping positions[{index}] must be an object"))
                })?,
                "positions",
            )
        })
        .collect()
}

fn tap_items(mapping: &Map<String, Value>) -> Result<Vec<TapItem>, KeymapError> {
    let values = mapping
        .get("items")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= MAX_PATH_POINTS)
        .ok_or_else(|| {
            KeymapError::Invalid(format!(
                "mapping field items must contain 1 to {MAX_PATH_POINTS} entries"
            ))
        })?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let item = value.as_object().ok_or_else(|| {
                KeymapError::Invalid(format!("mapping items[{index}] must be an object"))
            })?;
            Ok(TapItem {
                position: position(item, "position")?,
                duration_ms: finite_number(item, "duration", 1.0, MAX_TIMING_MS)?,
                wait_ms: finite_number(item, "wait", 0.0, MAX_TIMING_MS)?,
            })
        })
        .collect()
}

fn finite_number(
    mapping: &Map<String, Value>,
    name: &str,
    minimum: f64,
    maximum: f64,
) -> Result<f64, KeymapError> {
    let value = mapping
        .get(name)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (minimum..=maximum).contains(value))
        .ok_or_else(|| {
            KeymapError::Invalid(format!(
                "mapping field {name} must be a finite number from {minimum} to {maximum}"
            ))
        })?;
    Ok(value)
}

fn bound(held: &BTreeSet<String>, keys: &[String]) -> bool {
    !keys.is_empty() && keys.iter().all(|key| held.contains(key))
}

fn claimed_binding_keys(active: bool, keys: &[String]) -> Vec<String> {
    if active { keys.to_vec() } else { Vec::new() }
}

fn clamp(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn validate_key_code(key: &str, context: &str) -> Result<(), KeymapError> {
    if key.is_empty()
        || key.len() > MAX_KEY_CODE_LENGTH
        || !key.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(KeymapError::Invalid(format!(
            "{context} must be an ASCII alphanumeric browser keyboard code"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::default_hardware_bindings;
    use serde_json::json;

    fn profile(mappings: Vec<Value>) -> KeyMappingProfile {
        KeyMappingProfile {
            version: 2,
            name: "game".into(),
            mappings,
            bundle_identifiers: Vec::new(),
            target_resolution: None,
            hardware_bindings: default_hardware_bindings(),
        }
    }

    #[test]
    fn dpad_uses_normalized_diagonal_motion() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "move",
                "type": "dpad",
                "contactId": 0,
                "x": 0.2,
                "y": 0.8,
                "radius": 0.1,
                "keys": { "up": "KeyW", "down": "KeyS", "left": "KeyA", "right": "KeyD" }
            })]),
            None,
        )
        .unwrap();
        let held = normalize_held_keys(vec!["KeyW".into(), "KeyD".into()]).unwrap();
        let frame = compiled.frame(&held, Duration::ZERO).unwrap();

        assert_eq!(frame.contacts.len(), 1);
        assert!((frame.contacts[0].x - (0.2 + 0.1 / std::f32::consts::SQRT_2)).abs() < 0.0001);
        assert!((frame.contacts[0].y - (0.8 - 0.1 / std::f32::consts::SQRT_2)).abs() < 0.0001);
    }

    #[test]
    fn ordinary_mappings_use_dynamic_leases_for_duplicate_pointer_ids() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![
                json!({
                    "id": "space", "type": "Press", "pointer_id": 0,
                    "position": { "x": 0.8, "y": 0.8 }, "bind": ["Space"]
                }),
                json!({
                    "id": "shift", "type": "Press", "pointer_id": 0,
                    "position": { "x": 0.7, "y": 0.8 }, "bind": ["ShiftLeft"]
                }),
                json!({
                    "id": "move", "type": "DirectionPad", "pointer_id": 1,
                    "position": { "x": 0.2, "y": 0.8 },
                    "bind": { "type": "Button", "up": ["KeyW"], "down": ["KeyS"], "left": ["KeyA"], "right": ["KeyD"] },
                    "max_offset_x": 100, "max_offset_y": 100
                }),
            ]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let held =
            normalize_key_state(vec!["Space".into(), "ShiftLeft".into(), "KeyW".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();

        let first = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(first.contacts.len(), 3);
        assert_eq!(first.active_mapping_ids, vec!["space", "shift", "move"]);
        assert_eq!(
            first
                .contacts
                .iter()
                .map(|contact| contact.identity)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(first.unavailable_mapping_ids.is_empty());

        let second = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(16),
            )
            .unwrap();
        assert_eq!(
            second
                .contacts
                .iter()
                .map(|contact| contact.identity)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        let shift_and_move = normalize_key_state(vec!["ShiftLeft".into(), "KeyW".into()]).unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &shift_and_move,
                &BTreeSet::new(),
                &[],
                now + Duration::from_millis(32),
            )
            .unwrap();
        let released_space = compiled
            .frame_with_runtime(
                &mut runtime,
                &shift_and_move,
                &BTreeMap::new(),
                now + Duration::from_millis(32),
            )
            .unwrap();
        assert_eq!(
            released_space
                .contacts
                .iter()
                .map(|contact| contact.identity)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let empty = BTreeSet::new();
        compiled
            .update_runtime(
                &mut runtime,
                &shift_and_move,
                &empty,
                &empty,
                &[],
                now + Duration::from_millis(48),
            )
            .unwrap();
        let released_all = compiled
            .frame_with_runtime(
                &mut runtime,
                &empty,
                &BTreeMap::new(),
                now + Duration::from_millis(48),
            )
            .unwrap();
        assert!(released_all.contacts.is_empty());
        assert!(runtime.mapping_contact_ids.is_empty());
    }

    #[test]
    fn fixed_pointer_mapping_reserves_its_identity_for_ordinary_mappings() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![
                json!({
                    "id": "look", "type": "Observation", "pointer_id": 0,
                    "position": { "x": 0.5, "y": 0.5 }, "bind": ["MouseRight"],
                    "max_radius": 100, "sensitivity_x": 1.0, "sensitivity_y": 1.0
                }),
                json!({
                    "id": "tap", "type": "Press", "pointer_id": 0,
                    "position": { "x": 0.8, "y": 0.8 }, "bind": ["Space"]
                }),
            ]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let held = normalize_key_state(vec!["MouseRight".into(), "Space".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();

        assert_eq!(frame.contacts.len(), 2);
        assert_eq!(frame.contacts[0].identity, 0);
        assert_eq!(frame.contacts[1].identity, 1);
        assert_eq!(frame.active_mapping_ids, vec!["look", "tap"]);
    }

    #[test]
    fn dynamic_lease_reports_unavailable_when_all_contacts_are_reserved() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "tap", "type": "Press", "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 }, "bind": ["Space"]
            })]),
            None,
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let held = normalize_key_state(vec!["Space".into()]).unwrap();
        let reserved = BTreeSet::from([0, 1, 2, 3, 4]);
        let frame = compiled
            .frame_with_runtime_and_reserved_contacts(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                Instant::now(),
                &reserved,
            )
            .unwrap();

        assert!(frame.contacts.is_empty());
        assert_eq!(frame.unavailable_mapping_ids, vec!["tap"]);
        assert!(runtime.mapping_contact_ids.is_empty());
    }

    #[test]
    fn single_tap_sync_keeps_contact_until_binding_release() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "sync-tap",
                "type": "SingleTap",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyF"],
                "duration": 25,
                "sync": true,
            })]),
            None,
        )
        .unwrap();
        let held = normalize_held_keys(vec!["KeyF".into()]).unwrap();
        let frame = compiled.frame(&held, Duration::from_secs(5)).unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!(frame.contacts[0].touching);
        assert!(
            compiled
                .frame(&BTreeSet::new(), Duration::ZERO)
                .unwrap()
                .contacts
                .is_empty()
        );
    }

    #[test]
    fn random_pointer_offset_is_bounded_and_reseeded_per_activation() {
        let mut mapping_profile = profile(vec![json!({
            "id": "tap",
            "type": "SingleTap",
            "pointer_id": 0,
            "position": { "x": 0.5, "y": 0.5 },
            "bind": ["KeyF"],
            "duration": 100,
            "random_offset_x": 100,
            "random_offset_y": 50,
            "sync": true,
        })]);
        mapping_profile.target_resolution = Some(KeyMappingResolution {
            width: 1000,
            height: 500,
        });
        let compiled = CompiledKeymap::from_profile(&mapping_profile, None).unwrap();
        let held = normalize_key_state(vec!["KeyF".into()]).unwrap();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState {
            random_seed: 0x1234_5678_9abc_def0,
            ..Default::default()
        };

        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        let first = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap()
            .contacts[0];
        assert!((0.4..=0.6).contains(&first.x));
        assert!((0.4..=0.6).contains(&first.y));

        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now + Duration::from_millis(1),
            )
            .unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &held,
                &held,
                &[],
                now + Duration::from_millis(2),
            )
            .unwrap();
        let second = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(2),
            )
            .unwrap()
            .contacts[0];
        assert_ne!(first, second);
    }

    #[test]
    fn randomized_swipe_curves_between_endpoints() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "swipe",
                "type": "Swipe",
                "pointer_id": 0,
                "position": { "x": 0.1, "y": 0.5 },
                "bind": ["KeyS"],
                "duration": 100,
                "enable_randomization": true,
                "positions": [
                    { "x": 0.1, "y": 0.5 },
                    { "x": 0.9, "y": 0.5 }
                ]
            })]),
            None,
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyS".into()]).unwrap();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState {
            random_seed: 0x0fed_cba9_8765_4321,
            ..Default::default()
        };
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        let bend = runtime.random_states["swipe"].path_bend;
        let frame = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(50),
            )
            .unwrap();

        assert_eq!(frame.contacts.len(), 1);
        assert!((frame.contacts[0].x - (0.5 + bend.x)).abs() < 0.0001);
        assert!((frame.contacts[0].y - (0.5 + bend.y)).abs() < 0.0001);
    }

    #[test]
    fn randomized_direction_pad_uses_anchor_and_distance_scale() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "stick",
                "type": "DirectionPad",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": { "type": "Button", "up": [], "down": [], "left": [], "right": ["KeyD"] },
                "max_offset_x": 100,
                "max_offset_y": 100,
                "enable_randomization": true,
                "random_offset_x": 20,
                "random_offset_y": 10,
                "random_distance_min_scale": 0.8,
                "random_distance_max_scale": 1.2,
                "jitter_offset_x": 0,
                "jitter_offset_y": 0,
                "initial_duration": 0
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyD".into()]).unwrap();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState {
            random_seed: 0x1111_2222_3333_4444,
            ..Default::default()
        };
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        let state = runtime.random_states["stick"];
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();

        assert_eq!(frame.contacts.len(), 1);
        let expected_x = 0.5 + state.offset.x + 0.1 * state.distance_scale;
        let expected_y = 0.5 + state.offset.y;
        assert!((frame.contacts[0].x - expected_x).abs() < 0.0001);
        assert!((frame.contacts[0].y - expected_y).abs() < 0.0001);
        assert!((0.8..=1.2).contains(&state.distance_scale));
        assert!(state.offset.x.abs() <= 0.02);
        assert!(state.offset.y.abs() <= 0.02);
    }

    #[test]
    fn joystick_direction_binding_uses_gamepad_axes() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "stick",
                "type": "DirectionPad",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": { "type": "JoyStick", "x": "LeftStickX", "y": "LeftStickY" },
                "max_offset_x": 100,
                "max_offset_y": 100,
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let axes = BTreeMap::from([
            (String::from("LeftStickX"), 1.0),
            (String::from("LeftStickY"), -0.5),
        ]);
        compiled
            .update_runtime_with_gamepad(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                &axes,
                now,
            )
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &BTreeSet::new(), &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        let scale = 1.0 / 1.25_f32.sqrt();
        assert!((frame.contacts[0].x - (0.5 + 0.1 * scale)).abs() < 0.0001);
        assert!((frame.contacts[0].y - (0.5 - 0.05 * scale)).abs() < 0.0001);
        assert_eq!(frame.active_mapping_ids, vec!["stick"]);
    }

    #[test]
    fn direction_pad_applies_up_boost_binding() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "move",
                "type": "DirectionPad",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": { "type": "Button", "up": ["KeyW"], "down": [], "left": [], "right": [] },
                "max_offset_x": 100,
                "max_offset_y": 100,
                "up_boost_key": ["ShiftLeft"],
                "up_boost_scale": 2.0,
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let up = normalize_key_state(vec!["KeyW".into()]).unwrap();
        let boosted = normalize_key_state(vec!["KeyW".into(), "ShiftLeft".into()]).unwrap();
        let normal = compiled.frame(&up, Duration::ZERO).unwrap();
        let fast = compiled.frame(&boosted, Duration::ZERO).unwrap();
        assert!((normal.contacts[0].y - 0.4).abs() < 0.0001);
        assert!((fast.contacts[0].y - 0.3).abs() < 0.0001);
    }

    #[test]
    fn direction_pad_initial_duration_applies_without_randomization() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "move",
                "type": "DirectionPad",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": { "type": "Button", "up": ["KeyW"], "down": [], "left": [], "right": [] },
                "max_offset_x": 100,
                "max_offset_y": 100,
                "initial_duration": 100,
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyW".into()]).unwrap();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState::default();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        let frame = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(50),
            )
            .unwrap();
        assert!((frame.contacts[0].y - 0.45).abs() < 0.0001);
    }

    #[test]
    fn mouse_cast_projects_absolute_cursor_through_cast_center() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "cast",
                "type": "MouseCastSpell",
                "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "center": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyQ"],
                "cast_radius": 200,
                "drag_radius": 100,
                "cast_no_direction": false,
                "horizontal_scale_factor": 1.0,
                "vertical_scale_factor": 1.0,
                "initial_duration": 0,
                "release_mode": "OnRelease",
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyQ".into()]).unwrap();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState::default();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &held,
                &held,
                &[KeymapPointerDelta {
                    mapping_id: "cast",
                    delta_x: 0.0,
                    delta_y: 0.0,
                    cursor_x: Some(0.7),
                    cursor_y: Some(0.5),
                }],
                now,
            )
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!((frame.contacts[0].x - 0.6).abs() < 0.0001);
        assert!((frame.contacts[0].y - 0.5).abs() < 0.0001);

        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &held,
                &BTreeSet::new(),
                &[KeymapPointerDelta {
                    mapping_id: "cast",
                    delta_x: -50.0,
                    delta_y: 0.0,
                    cursor_x: None,
                    cursor_y: None,
                }],
                now,
            )
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert!((frame.contacts[0].x - 0.55).abs() < 0.0001);
    }

    #[test]
    fn joystick_deadzone_does_not_create_a_contact() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "stick", "type": "DirectionPad", "pointer_id": 0,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": { "type": "JoyStick", "x": "LeftStickX", "y": "LeftStickY" },
                "max_offset_x": 100, "max_offset_y": 100
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 1000,
            }),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        compiled
            .update_runtime_with_gamepad(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                &BTreeMap::from([("LeftStickX".into(), 0.04), ("LeftStickY".into(), -0.03)]),
                now,
            )
            .unwrap();
        assert!(
            compiled
                .frame_with_runtime(&mut runtime, &BTreeSet::new(), &BTreeMap::new(), now)
                .unwrap()
                .contacts
                .is_empty()
        );
    }

    #[test]
    fn pulse_swipe_finishes_after_key_release() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "swipe",
                "type": "Swipe",
                "pointer_id": 0,
                "position": { "x": 0.1, "y": 0.5 },
                "bind": ["KeyS"],
                "duration": 100,
                "positions": [
                    { "x": 0.1, "y": 0.5 },
                    { "x": 0.9, "y": 0.5 }
                ],
            })]),
            None,
        )
        .unwrap();
        let pressed = normalize_key_state(vec!["KeyS".into()]).unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &pressed, &pressed, &[], now)
            .unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &pressed,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now + Duration::from_millis(10),
            )
            .unwrap();
        let middle = compiled
            .frame_with_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeMap::new(),
                now + Duration::from_millis(50),
            )
            .unwrap();
        assert_eq!(middle.contacts.len(), 1);
        assert!((middle.contacts[0].x - 0.5).abs() < 0.0001);
        let finished = compiled
            .frame_with_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeMap::new(),
                now + Duration::from_millis(110),
            )
            .unwrap();
        assert!(finished.contacts.is_empty());
    }

    #[test]
    fn unsupported_mapping_fails_only_when_its_binding_is_triggered() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "raw",
                "type": "RawInput",
                "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyR"]
            })]),
            None,
        )
        .unwrap();
        let held = normalize_held_keys(vec!["KeyR".into()]).unwrap();

        assert!(matches!(
            compiled.frame(&held, Duration::ZERO),
            Err(KeymapError::Unsupported { .. })
        ));
    }

    #[test]
    fn press_stays_down_for_the_complete_held_state() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "pickup",
                "type": "Press",
                "pointer_id": 4,
                "position": { "x": 0.63, "y": 0.55 },
                "bind": ["KeyF"]
            })]),
            None,
        )
        .unwrap();
        let held = normalize_held_keys(vec!["KeyF".into()]).unwrap();

        let initial = compiled.frame(&held, Duration::ZERO).unwrap();
        let later = compiled.frame(&held, Duration::from_secs(8)).unwrap();
        let released = compiled
            .frame(&BTreeSet::new(), Duration::from_secs(8))
            .unwrap();

        assert_eq!(initial.contacts.len(), 1);
        assert!(initial.contacts[0].touching);
        assert_eq!(initial.contacts, later.contacts);
        assert!(released.contacts.is_empty());
    }

    #[test]
    fn persistent_hold_times_do_not_restart_repeat_taps() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "fire",
                "type": "RepeatTap",
                "pointer_id": 1,
                "position": { "x": 0.75, "y": 0.25 },
                "bind": ["Space"],
                "duration": 40,
                "interval": 60,
            })]),
            None,
        )
        .unwrap();
        let held = normalize_key_state(vec!["Space".into()]).unwrap();
        let held_for = BTreeMap::from([(String::from("Space"), Duration::from_millis(70))]);
        let mut runtime = KeymapRuntimeState::default();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &held_for, Instant::now())
            .unwrap();
        assert!(frame.contacts.is_empty());

        let held_for = BTreeMap::from([(String::from("Space"), Duration::from_millis(110))]);
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &held_for, Instant::now())
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!(frame.contacts[0].touching);
    }

    #[test]
    fn fps_mapping_toggles_and_accepts_pointer_deltas_after_key_release() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "aim",
                "type": "Fps",
                "pointer_id": 2,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyQ"],
                "sensitivity_x": 1.0,
                "sensitivity_y": 0.5,
                "max_offset_x": 200,
                "max_offset_y": 200,
                "touch_mode": { "type": "single", "interval": 0 },
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyQ".into()]).unwrap();
        let newly_held = held.clone();
        let now = Instant::now();
        let mut runtime = KeymapRuntimeState::default();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &held,
                &newly_held,
                &[KeymapPointerDelta {
                    mapping_id: "aim",
                    delta_x: 100.0,
                    delta_y: 100.0,
                    cursor_x: None,
                    cursor_y: None,
                }],
                now,
            )
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!((frame.contacts[0].x - 0.6).abs() < 0.0001);
        assert!((frame.contacts[0].y - 0.6).abs() < 0.0001);

        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now,
            )
            .unwrap();
        let released_key = compiled
            .frame_with_runtime(&mut runtime, &BTreeSet::new(), &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(released_key.contacts.len(), 1);

        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        assert!(
            compiled
                .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
                .unwrap()
                .contacts
                .is_empty()
        );
    }

    #[test]
    fn cancel_cast_releases_active_mouse_cast_mappings() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![
                json!({
                    "id": "cast",
                    "type": "MouseCastSpell",
                    "pointer_id": 1,
                    "position": { "x": 0.6, "y": 0.5 },
                    "bind": ["KeyQ"],
                    "horizontal_scale_factor": 1.0,
                    "vertical_scale_factor": 1.0,
                    "drag_radius": 100,
                    "cast_no_direction": false,
                    "initial_duration": 0,
                    "release_mode": "OnRelease",
                }),
                json!({
                    "id": "cancel",
                    "type": "CancelCast",
                    "position": { "x": 0.5, "y": 0.5 },
                    "bind": ["Escape"],
                }),
            ]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let now = Instant::now();
        let cast_held = normalize_key_state(vec!["KeyQ".into()]).unwrap();
        let mut runtime = KeymapRuntimeState::default();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &cast_held,
                &cast_held,
                &[],
                now,
            )
            .unwrap();
        let held = normalize_key_state(vec!["KeyQ".into(), "Escape".into()]).unwrap();
        let newly = normalize_key_state(vec!["Escape".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &cast_held, &held, &newly, &[], now)
            .unwrap();
        let animating = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(75),
            )
            .unwrap();
        assert_eq!(animating.active_mapping_ids, vec!["cancel"]);
        assert!((animating.contacts[0].x - 0.55).abs() < 0.0001);
        let released = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(150),
            )
            .unwrap();
        assert!(released.contacts.is_empty());
    }

    #[test]
    fn observation_clamps_pointer_motion_and_releases_with_its_binding() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "look", "type": "Observation", "pointer_id": 1,
                "position": { "x": 0.5, "y": 0.5 }, "bind": ["KeyO"],
                "sensitivity_x": 1.0, "sensitivity_y": 1.0, "max_radius": 20,
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyO".into()]).unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &held,
                &held,
                &[KeymapPointerDelta {
                    mapping_id: "look",
                    delta_x: 100.0,
                    delta_y: 0.0,
                    cursor_x: None,
                    cursor_y: None,
                }],
                now,
            )
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert!((frame.contacts[0].x - 0.52).abs() < 0.0001);
        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now,
            )
            .unwrap();
        assert!(
            compiled
                .frame_with_runtime(&mut runtime, &BTreeSet::new(), &BTreeMap::new(), now)
                .unwrap()
                .contacts
                .is_empty()
        );
    }

    #[test]
    fn non_preserving_fire_temporarily_takes_over_fps_control() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![
                json!({
                    "id": "camera", "type": "Fps", "pointer_id": 0,
                    "position": { "x": 0.5, "y": 0.5 }, "bind": ["KeyV"],
                    "sensitivity_x": 1.0, "sensitivity_y": 1.0,
                    "max_offset_x": 200, "max_offset_y": 200,
                    "touch_mode": { "type": "single", "interval": 0 },
                }),
                json!({
                    "id": "fire", "type": "Fire", "pointer_id": 2,
                    "position": { "x": 0.8, "y": 0.7 }, "bind": ["MouseLeft"],
                    "sensitivity_x": 1.0, "sensitivity_y": 1.0,
                    "preserve_fps_control": false,
                }),
            ]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let fps_key = normalize_key_state(vec!["KeyV".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &fps_key, &fps_key, &[], now)
            .unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &fps_key,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now,
            )
            .unwrap();
        let fire_key = normalize_key_state(vec!["MouseLeft".into()]).unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &fire_key,
                &fire_key,
                &[KeymapPointerDelta {
                    mapping_id: "fire",
                    delta_x: 10.0,
                    delta_y: 5.0,
                    cursor_x: None,
                    cursor_y: None,
                }],
                now,
            )
            .unwrap();
        let firing = compiled
            .frame_with_runtime(&mut runtime, &fire_key, &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(firing.contacts.len(), 1);
        assert_eq!(firing.contacts[0].identity, 2);
        assert!((firing.contacts[0].x - 0.81).abs() < 0.0001);
        compiled
            .update_runtime(
                &mut runtime,
                &fire_key,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now,
            )
            .unwrap();
        let restored = compiled
            .frame_with_runtime(&mut runtime, &BTreeSet::new(), &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(
            restored.contacts,
            vec![NormalizedTouchContact {
                identity: 0,
                touching: true,
                x: 0.5,
                y: 0.5
            }]
        );
    }

    fn script_options() -> CompileOptions {
        CompileOptions {
            allow_scripts: true,
        }
    }

    fn script_frame() -> KeyMappingResolution {
        KeyMappingResolution {
            width: 1000,
            height: 500,
        }
    }

    #[test]
    fn script_mapping_runs_pressed_held_and_released_without_overlap() {
        let compiled = CompiledKeymap::from_profile_with_options(
            &profile(vec![json!({
                "id": "macro", "type": "Script", "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyM"], "interval": 20,
                "pressed_script": "state_set(\"pressed\", true)",
                "held_script": "tap(1, 100, 200)",
                "released_script": "if state_get(\"pressed\", false) { paste_text(\"done\") }"
            })]),
            Some(script_frame()),
            script_options(),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let held = normalize_key_state(vec!["KeyM".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();

        let down = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert_eq!(down.contacts.len(), 1);
        assert_eq!(down.contacts[0].identity, 1);

        let released_touch = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(30),
            )
            .unwrap();
        assert!(released_touch.contacts.is_empty());

        let next_held = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(50),
            )
            .unwrap();
        assert_eq!(next_held.contacts.len(), 1);

        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now + Duration::from_millis(51),
            )
            .unwrap();
        let release = compiled
            .frame_with_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeMap::new(),
                now + Duration::from_millis(80),
            )
            .unwrap();
        assert!(
            release
                .script_actions
                .iter()
                .any(|action| matches!(action, ScriptAction::Text { text } if text == "done"))
        );
    }

    #[test]
    fn before_script_waits_before_the_mapping_contact_becomes_active() {
        let profile = profile(vec![json!({
            "id": "delayed", "type": "Press", "pointer_id": 1,
            "position": { "x": 0.25, "y": 0.75 }, "bind": ["KeyT"],
            "script_hooks": {
                "before_script": "paste_text(\"ready\"); wait(100)",
                "after_script": ""
            }
        })]);
        validate_profile_scripts(&profile).unwrap();
        let compiled = CompiledKeymap::from_profile_with_options(
            &profile,
            Some(script_frame()),
            script_options(),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let held = normalize_key_state(vec!["KeyT".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();

        let waiting = compiled
            .frame_with_runtime(&mut runtime, &held, &BTreeMap::new(), now)
            .unwrap();
        assert!(waiting.contacts.is_empty());
        assert!(
            waiting
                .script_actions
                .iter()
                .any(|action| matches!(action, ScriptAction::Text { text } if text == "ready"))
        );

        let still_waiting = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(99),
            )
            .unwrap();
        assert!(still_waiting.contacts.is_empty());

        let ready = compiled
            .frame_with_runtime(
                &mut runtime,
                &held,
                &BTreeMap::new(),
                now + Duration::from_millis(100),
            )
            .unwrap();
        assert_eq!(ready.active_mapping_ids, vec!["delayed"]);
        assert_eq!(ready.contacts.len(), 1);
    }

    #[test]
    fn releasing_during_before_script_cancels_persistent_activation() {
        let compiled = CompiledKeymap::from_profile_with_options(
            &profile(vec![json!({
                "id": "camera", "type": "Fps", "pointer_id": 2,
                "position": { "x": 0.5, "y": 0.5 }, "bind": ["KeyV"],
                "sensitivity_x": 1.0, "sensitivity_y": 1.0,
                "max_offset_x": 200, "max_offset_y": 200,
                "touch_mode": { "type": "single", "interval": 0 },
                "script_hooks": {
                    "before_script": "wait(100)",
                    "after_script": ""
                }
            })]),
            Some(script_frame()),
            script_options(),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let held = normalize_key_state(vec!["KeyV".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &held, &held, &[], now)
            .unwrap();
        compiled
            .update_runtime(
                &mut runtime,
                &held,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &[],
                now + Duration::from_millis(50),
            )
            .unwrap();

        let frame = compiled
            .frame_with_runtime(
                &mut runtime,
                &BTreeSet::new(),
                &BTreeMap::new(),
                now + Duration::from_millis(100),
            )
            .unwrap();
        assert!(frame.contacts.is_empty());
        assert!(!frame.active_mapping_ids.iter().any(|id| id == "camera"));
    }

    #[test]
    fn profile_script_validation_rejects_invalid_source_without_running_it() {
        let invalid = profile(vec![json!({
            "id": "macro", "type": "Script", "position": { "x": 0.5, "y": 0.5 },
            "bind": ["KeyM"], "interval": 20,
            "pressed_script": "if {", "held_script": "", "released_script": ""
        })]);

        assert!(validate_profile_scripts(&invalid).is_err());
    }

    #[test]
    fn script_hooks_emit_commands_and_script_modes_share_advanced_runtime() {
        let compiled = CompiledKeymap::from_profile_with_options(
            &profile(vec![
                json!({
                    "id": "tap", "type": "SingleTap", "pointer_id": 1,
                    "position": { "x": 0.2, "y": 0.2 }, "bind": ["KeyT"],
                    "duration": 30, "sync": false,
                    "script_hooks": {
                        "before_script": "paste_text(\"before\")",
                        "after_script": "paste_text(\"after\")"
                    }
                }),
                json!({
                    "id": "camera", "type": "Fps", "pointer_id": 2,
                    "position": { "x": 0.5, "y": 0.5 }, "bind": ["KeyV"],
                    "sensitivity_x": 1.0, "sensitivity_y": 1.0,
                    "max_offset_x": 200, "max_offset_y": 200,
                    "touch_mode": { "type": "single", "interval": 0 }
                }),
                json!({
                    "id": "mode", "type": "Script", "position": { "x": 0.5, "y": 0.5 },
                    "bind": ["KeyF"], "interval": 100,
                    "pressed_script": "enter_fps(\"camera\")",
                    "held_script": "", "released_script": ""
                }),
            ]),
            Some(script_frame()),
            script_options(),
        )
        .unwrap();
        let mut runtime = KeymapRuntimeState::default();
        let now = Instant::now();
        let tap = normalize_key_state(vec!["KeyT".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &tap, &tap, &[], now)
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &tap, &BTreeMap::new(), now)
            .unwrap();
        assert!(
            frame
                .script_actions
                .iter()
                .any(|action| matches!(action, ScriptAction::Text { text } if text == "before"))
        );

        let mode = normalize_key_state(vec!["KeyF".into()]).unwrap();
        compiled
            .update_runtime(&mut runtime, &BTreeSet::new(), &mode, &mode, &[], now)
            .unwrap();
        let frame = compiled
            .frame_with_runtime(&mut runtime, &mode, &BTreeMap::new(), now)
            .unwrap();
        assert!(frame.contacts.iter().any(|contact| contact.identity == 2));
        assert!(frame.active_mapping_ids.iter().any(|id| id == "camera"));
    }
}
