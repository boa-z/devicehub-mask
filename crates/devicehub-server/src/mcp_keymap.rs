//! Deterministic key-mapping playback shared by the MCP adapter.
//!
//! The desktop frontend remains the interactive mapping runtime. This module
//! evaluates the deterministic subset of saved mappings, including agent-supplied
//! pointer deltas. It intentionally never executes browser script hooks.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::time::Duration;

use devicehub_core::{HardwareButton, KeyMappingProfile, KeyMappingResolution, hardware_button};
use serde_json::{Map, Value};

const MAX_KEY_CODES: usize = 32;
const MAX_KEY_CODE_LENGTH: usize = 64;
const MAX_TIMING_MS: f64 = 60_000.0;
const MAX_PATH_POINTS: usize = 32;
const MAX_POINTER_SENSITIVITY: f64 = 100.0;
const MAX_POINTER_DELTA: f32 = 16_384.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NormalizedTouchContact {
    pub identity: u8,
    pub touching: bool,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KeymapFrame {
    pub contacts: Vec<NormalizedTouchContact>,
    pub active_mapping_ids: Vec<String>,
    pub matched_mapping_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveHardwareButton {
    pub name: String,
    pub button: HardwareButton,
}

/// Relative pointer movement in pixels of the profile's target display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KeymapPointerDelta<'a> {
    pub mapping_id: &'a str,
    pub delta_x: f32,
    pub delta_y: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledKeymap {
    mappings: Vec<Mapping>,
    hardware_bindings: Vec<HardwareBinding>,
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
    },
    Press {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
    },
    RepeatTap {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        duration_ms: f64,
        interval_ms: f64,
    },
    MultipleTap {
        id: String,
        identity: u8,
        bind: Vec<String>,
        items: Vec<TapItem>,
    },
    Swipe {
        id: String,
        identity: u8,
        bind: Vec<String>,
        positions: Vec<Point>,
        duration_ms: f64,
    },
    DirectionPad {
        id: String,
        identity: u8,
        position: Point,
        binding: DirectionBinding,
        max_offset_x: f32,
        max_offset_y: f32,
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
    },
    Pointer {
        id: String,
        identity: u8,
        position: Point,
        bind: Vec<String>,
        sensitivity_x: f32,
        sensitivity_y: f32,
        frame: KeyMappingResolution,
        is_cast: bool,
    },
    CancelCast {
        id: String,
        bind: Vec<String>,
    },
    Unsupported {
        id: String,
        mapping_type: String,
        activation: Activation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone)]
struct TapItem {
    position: Point,
    duration_ms: f64,
    wait_ms: f64,
}

struct PointerParts<'a> {
    id: &'a str,
    position: Point,
    bind: &'a [String],
    sensitivity_x: f32,
    sensitivity_y: f32,
    frame: KeyMappingResolution,
}

#[derive(Debug, Clone)]
enum DirectionBinding {
    Button {
        up: Vec<String>,
        down: Vec<String>,
        left: Vec<String>,
        right: Vec<String>,
    },
    JoyStick,
}

#[derive(Debug, Clone)]
enum Activation {
    All(Vec<String>),
    Never,
}

#[derive(Debug, Clone)]
pub(crate) enum KeymapError {
    Invalid(String),
    Unsupported { id: String, mapping_type: String },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Unsupported { id, mapping_type } => write!(
                formatter,
                "mapping {id} uses {mapping_type}, which MCP keymap playback does not support"
            ),
        }
    }
}

impl std::error::Error for KeymapError {}

impl CompiledKeymap {
    pub(crate) fn from_profile(
        profile: &KeyMappingProfile,
        fallback_frame: Option<KeyMappingResolution>,
    ) -> Result<Self, KeymapError> {
        let frame = profile.target_resolution.or(fallback_frame);
        let mappings = profile
            .mappings
            .iter()
            .map(|mapping| compile_mapping(mapping, frame))
            .collect::<Result<Vec<_>, _>>()?;
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
        })
    }

    pub(crate) fn frame(
        &self,
        held: &BTreeSet<String>,
        elapsed: Duration,
    ) -> Result<KeymapFrame, KeymapError> {
        self.frame_with_state(held, &BTreeMap::new(), elapsed, &BTreeMap::new())
    }

    /// Evaluate a persistent game session. `held_for` tracks each key from its
    /// most recent press so a keepalive does not restart tap and swipe mappings.
    pub(crate) fn frame_with_hold_times(
        &self,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
        pointer_offsets: &BTreeMap<String, (f32, f32)>,
    ) -> Result<KeymapFrame, KeymapError> {
        self.frame_with_state(held, held_for, Duration::ZERO, pointer_offsets)
    }

    fn frame_with_state(
        &self,
        held: &BTreeSet<String>,
        held_for: &BTreeMap<String, Duration>,
        fallback_elapsed: Duration,
        pointer_offsets: &BTreeMap<String, (f32, f32)>,
    ) -> Result<KeymapFrame, KeymapError> {
        let held = self.effective_held(held);
        let mut candidates = Vec::with_capacity(self.mappings.len());
        let mut matched_mapping_ids = Vec::new();
        for mapping in &self.mappings {
            let elapsed = mapping
                .activation_elapsed(&held, held_for)
                .unwrap_or(fallback_elapsed);
            let evaluated =
                mapping.evaluate(&held, elapsed.as_secs_f64() * 1000.0, pointer_offsets)?;
            if evaluated.matched {
                matched_mapping_ids.push(evaluated.id.clone());
            }
            candidates.push(evaluated);
        }

        let mut claimed_keys = HashSet::new();
        let mut contacts = Vec::new();
        let mut active_mapping_ids = Vec::new();
        for candidate in candidates {
            let Some(contact) = candidate.contact else {
                continue;
            };
            if !contact.touching
                || candidate
                    .claimed_keys
                    .iter()
                    .any(|key| claimed_keys.contains(key))
            {
                continue;
            }
            claimed_keys.extend(candidate.claimed_keys);
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

        Ok(KeymapFrame {
            contacts,
            active_mapping_ids,
            matched_mapping_ids,
        })
    }

    pub(crate) fn active_hardware_buttons(
        &self,
        held: &BTreeSet<String>,
    ) -> Vec<ActiveHardwareButton> {
        self.hardware_bindings
            .iter()
            .filter(|binding| held.contains(&binding.key))
            .map(|binding| ActiveHardwareButton {
                name: binding.name.clone(),
                button: binding.button,
            })
            .collect()
    }

    pub(crate) fn reset_pointer_offsets_for_new_keys(
        &self,
        newly_held: &BTreeSet<String>,
        pointer_offsets: &mut BTreeMap<String, (f32, f32)>,
    ) {
        if newly_held.is_empty() {
            return;
        }
        for mapping in &self.mappings {
            let Some(pointer) = mapping.pointer_parts() else {
                continue;
            };
            if pointer.bind.iter().any(|key| newly_held.contains(key)) {
                pointer_offsets.remove(pointer.id);
            }
        }
    }

    pub(crate) fn apply_pointer_deltas(
        &self,
        held: &BTreeSet<String>,
        pointer_offsets: &mut BTreeMap<String, (f32, f32)>,
        deltas: &[KeymapPointerDelta<'_>],
    ) -> Result<(), KeymapError> {
        let held = self.effective_held(held);
        for delta in deltas {
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
            let mapping = self
                .mappings
                .iter()
                .find(|mapping| mapping.id() == delta.mapping_id)
                .ok_or_else(|| {
                    KeymapError::Invalid(format!("unknown pointer mapping: {}", delta.mapping_id))
                })?;
            let Some(pointer) = mapping.pointer_parts() else {
                return Err(KeymapError::Invalid(format!(
                    "mapping {} does not accept pointer deltas",
                    delta.mapping_id
                )));
            };
            if !bound(&held, pointer.bind) {
                return Err(KeymapError::Invalid(format!(
                    "mapping {} must be held before applying a pointer delta",
                    delta.mapping_id
                )));
            }
            let (x, y) = pointer_offsets
                .get(pointer.id)
                .copied()
                .unwrap_or((pointer.position.x, pointer.position.y));
            pointer_offsets.insert(
                pointer.id.to_string(),
                (
                    clamp(x + delta.delta_x * pointer.sensitivity_x / pointer.frame.width as f32),
                    clamp(y + delta.delta_y * pointer.sensitivity_y / pointer.frame.height as f32),
                ),
            );
        }
        Ok(())
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
}

impl Mapping {
    fn evaluate(
        &self,
        held: &BTreeSet<String>,
        elapsed_ms: f64,
        pointer_offsets: &BTreeMap<String, (f32, f32)>,
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
                let (dx, dy) = binding.direction(held);
                let matched = binding.has_pressed_key(held);
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
                        binding.pressed_keys(held)
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
            } => {
                let matched = bound(held, bind);
                let touching = matched && elapsed_ms < *duration_ms;
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    *position,
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
            } => {
                let matched = bound(held, bind);
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    *position,
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
            } => {
                let matched = bound(held, bind);
                let period = (*duration_ms + *interval_ms).max(1.0);
                let touching = matched && elapsed_ms % period < *duration_ms;
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    *position,
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
            } => {
                let matched = bound(held, bind);
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
            } => {
                let matched = bound(held, bind);
                let progress = (elapsed_ms / duration_ms.max(1.0)).min(1.0);
                let segment = progress * (positions.len() - 1) as f64;
                let index = segment.floor() as usize;
                let next = (index + 1).min(positions.len() - 1);
                let amount = (segment - index as f64) as f32;
                let start = positions[index];
                let end = positions[next];
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: start.x + (end.x - start.x) * amount,
                        y: start.y + (end.y - start.y) * amount,
                    },
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
                frame,
            } => {
                let (dx, dy) = binding.direction(held);
                let matched = binding.has_pressed_key(held);
                let touching = dx != 0.0 || dy != 0.0;
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: clamp(position.x + dx * max_offset_x / frame.width as f32),
                        y: clamp(position.y + dy * max_offset_y / frame.height as f32),
                    },
                    touching,
                    if touching {
                        binding.pressed_keys(held)
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
            } => {
                let matched = bound(held, bind);
                let (dx, dy) = pad_binding.direction(held);
                Ok(EvaluatedMapping::contact(
                    id,
                    *identity,
                    Point {
                        x: clamp(position.x + dx * drag_radius / frame.width as f32),
                        y: clamp(position.y + dy * drag_radius / frame.height as f32),
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
                let (x, y) = pointer_offsets
                    .get(id)
                    .copied()
                    .unwrap_or((position.x, position.y));
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
            Self::CancelCast { id, bind } => {
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

    fn pointer_parts(&self) -> Option<PointerParts<'_>> {
        let Self::Pointer {
            id,
            position,
            bind,
            sensitivity_x,
            sensitivity_y,
            frame,
            ..
        } = self
        else {
            return None;
        };
        Some(PointerParts {
            id,
            position: *position,
            bind,
            sensitivity_x: *sensitivity_x,
            sensitivity_y: *sensitivity_y,
            frame: *frame,
        })
    }

    fn cast_binding(&self) -> Option<&[String]> {
        match self {
            Self::PadCastSpell { bind, .. } => Some(bind),
            Self::Pointer {
                bind,
                is_cast: true,
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
    fn direction(&self, held: &BTreeSet<String>) -> (f32, f32) {
        let Self::Button {
            up,
            down,
            left,
            right,
        } = self
        else {
            return (0.0, 0.0);
        };
        let mut dx = f32::from(bound(held, right)) - f32::from(bound(held, left));
        let mut dy = f32::from(bound(held, down)) - f32::from(bound(held, up));
        if dx != 0.0 && dy != 0.0 {
            dx /= std::f32::consts::SQRT_2;
            dy /= std::f32::consts::SQRT_2;
        }
        (dx, dy)
    }

    fn has_pressed_key(&self, held: &BTreeSet<String>) -> bool {
        self.pressed_keys(held).into_iter().next().is_some()
    }

    fn pressed_keys(&self, held: &BTreeSet<String>) -> Vec<String> {
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

impl Activation {
    fn is_active(&self, held: &BTreeSet<String>) -> bool {
        match self {
            Self::All(keys) => bound(held, keys),
            Self::Never => false,
        }
    }
}

pub(crate) fn normalize_held_keys(keys: Vec<String>) -> Result<BTreeSet<String>, KeymapError> {
    if keys.is_empty() {
        return Err(KeymapError::Invalid(format!(
            "keys must contain between one and {MAX_KEY_CODES} browser keyboard codes"
        )));
    }
    normalize_key_state(keys)
}

/// Validate a complete key-state update. An empty state is valid and releases
/// all mapped controls in a persistent game session.
pub(crate) fn normalize_key_state(keys: Vec<String>) -> Result<BTreeSet<String>, KeymapError> {
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
        }),
        "Press" => Ok(Mapping::Press {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
        }),
        "RepeatTap" => Ok(Mapping::RepeatTap {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
            duration_ms: finite_number(mapping, "duration", 1.0, MAX_TIMING_MS)?,
            interval_ms: finite_number(mapping, "interval", 0.0, MAX_TIMING_MS)?,
        }),
        "MultipleTap" => Ok(Mapping::MultipleTap {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            bind: binding_field(mapping, "bind")?,
            items: tap_items(mapping)?,
        }),
        "Swipe" => Ok(Mapping::Swipe {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            bind: binding_field(mapping, "bind")?,
            positions: positions(mapping)?,
            duration_ms: finite_number(mapping, "duration", 1.0, MAX_TIMING_MS)?,
        }),
        "DirectionPad" => Ok(Mapping::DirectionPad {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            binding: direction_binding(mapping, "bind")?,
            max_offset_x: finite_number(mapping, "max_offset_x", 0.0, MAX_TIMING_MS)? as f32,
            max_offset_y: finite_number(mapping, "max_offset_y", 0.0, MAX_TIMING_MS)? as f32,
            frame: frame.ok_or_else(|| {
                KeymapError::Invalid(
                    "DirectionPad needs targetResolution or a connected device screen".into(),
                )
            })?,
        }),
        "PadCastSpell" => Ok(Mapping::PadCastSpell {
            id,
            identity: contact_id(mapping, "pointer_id")?,
            position: position(mapping, "position")?,
            bind: binding_field(mapping, "bind")?,
            pad_binding: direction_binding(mapping, "pad_bind")?,
            drag_radius: finite_number(mapping, "drag_radius", 0.0, MAX_TIMING_MS)? as f32,
            frame: frame.ok_or_else(|| {
                KeymapError::Invalid(
                    "PadCastSpell needs targetResolution or a connected device screen".into(),
                )
            })?,
        }),
        "MouseCastSpell" => pointer_mapping(
            id,
            mapping,
            frame,
            "MouseCastSpell",
            "horizontal_scale_factor",
            "vertical_scale_factor",
            true,
        ),
        "Observation" => pointer_mapping(
            id,
            mapping,
            frame,
            "Observation",
            "sensitivity_x",
            "sensitivity_y",
            false,
        ),
        "Fps" => pointer_mapping(
            id,
            mapping,
            frame,
            "Fps",
            "sensitivity_x",
            "sensitivity_y",
            false,
        ),
        "Fire" => pointer_mapping(
            id,
            mapping,
            frame,
            "Fire",
            "sensitivity_x",
            "sensitivity_y",
            false,
        ),
        "CancelCast" => Ok(Mapping::CancelCast {
            id,
            bind: binding_field(mapping, "bind")?,
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
    is_cast: bool,
) -> Result<Mapping, KeymapError> {
    Ok(Mapping::Pointer {
        id,
        identity: contact_id(mapping, "pointer_id")?,
        position: position(mapping, "position")?,
        bind: binding_field(mapping, "bind")?,
        sensitivity_x: finite_number(mapping, sensitivity_x, 0.0, MAX_POINTER_SENSITIVITY)? as f32,
        sensitivity_y: finite_number(mapping, sensitivity_y, 0.0, MAX_POINTER_SENSITIVITY)? as f32,
        frame: frame.ok_or_else(|| {
            KeymapError::Invalid(format!(
                "{mapping_type} needs targetResolution or a connected device screen"
            ))
        })?,
        is_cast,
    })
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
        "JoyStick" => Ok(DirectionBinding::JoyStick),
        _ => Err(KeymapError::Invalid(format!(
            "mapping field {name}.type must be Button or JoyStick"
        ))),
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
        let frame = compiled
            .frame_with_hold_times(&held, &held_for, &BTreeMap::new())
            .unwrap();
        assert!(frame.contacts.is_empty());

        let held_for = BTreeMap::from([(String::from("Space"), Duration::from_millis(110))]);
        let frame = compiled
            .frame_with_hold_times(&held, &held_for, &BTreeMap::new())
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!(frame.contacts[0].touching);
    }

    #[test]
    fn pointer_mappings_apply_agent_relative_deltas() {
        let compiled = CompiledKeymap::from_profile(
            &profile(vec![json!({
                "id": "aim",
                "type": "Fps",
                "pointer_id": 2,
                "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyQ"],
                "sensitivity_x": 1.0,
                "sensitivity_y": 0.5,
            })]),
            Some(KeyMappingResolution {
                width: 1000,
                height: 500,
            }),
        )
        .unwrap();
        let held = normalize_key_state(vec!["KeyQ".into()]).unwrap();
        let mut offsets = BTreeMap::new();
        compiled
            .apply_pointer_deltas(
                &held,
                &mut offsets,
                &[KeymapPointerDelta {
                    mapping_id: "aim",
                    delta_x: 100.0,
                    delta_y: 100.0,
                }],
            )
            .unwrap();
        let frame = compiled
            .frame_with_hold_times(&held, &BTreeMap::new(), &offsets)
            .unwrap();
        assert_eq!(frame.contacts.len(), 1);
        assert!((frame.contacts[0].x - 0.6).abs() < 0.0001);
        assert!((frame.contacts[0].y - 0.6).abs() < 0.0001);
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
        let held = normalize_key_state(vec!["KeyQ".into(), "Escape".into()]).unwrap();
        let frame = compiled.frame(&held, Duration::ZERO).unwrap();
        assert!(frame.contacts.is_empty());
        assert_eq!(frame.matched_mapping_ids, vec!["cancel"]);
    }
}
