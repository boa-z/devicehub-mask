use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use devicehub_core::{
    DeviceInputCommand, KeyMappingProfile, KeyMappingResolution, Orientation, TouchContact,
    hardware_button, norm, unrotate_norm, validate_key_mapping_profile,
};
use devicehub_keymap::{
    ActiveHardwareButton, CompileOptions, CompiledKeymap, KeymapPointerDelta, KeymapRuntimeState,
    NormalizedTouchContact, ScriptAction, normalize_key_state,
};
use devicehub_runtime::{DeviceSessionCommand as InputCmd, SessionCommandSlot as InputSink};

#[derive(Debug, Clone, Copy, Deserialize)]
pub(super) struct BrowserKeymapResolution {
    width: u32,
    height: u32,
}

impl From<BrowserKeymapResolution> for KeyMappingResolution {
    fn from(value: BrowserKeymapResolution) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct BrowserKeymapPointerDelta {
    mapping_id: String,
    delta_x: f32,
    delta_y: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(super) struct BrowserDirectContact {
    identity: u8,
    touching: bool,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Default)]
struct ScriptDeviceState {
    keyboard: BTreeSet<u64>,
    buttons: BTreeSet<String>,
}

#[derive(Default)]
pub(super) struct BrowserKeymapSession {
    compiled: Option<CompiledKeymap>,
    runtime: KeymapRuntimeState,
    held: BTreeSet<String>,
    held_since: BTreeMap<String, Instant>,
    direct_contacts: Vec<NormalizedTouchContact>,
    active_contacts: Vec<NormalizedTouchContact>,
    active_buttons: Vec<ActiveHardwareButton>,
    script_device: ScriptDeviceState,
    active_mapping_ids: Vec<String>,
    debug_enabled: bool,
}

impl BrowserKeymapSession {
    pub(super) fn configure<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
        profile: KeyMappingProfile,
        frame: BrowserKeymapResolution,
        allow_scripts: bool,
    ) -> Value {
        self.release(input, orientation);
        if validate_key_mapping_profile(&profile).is_err() {
            return error_event("keymap profile failed native v2 validation");
        }
        let frame = KeyMappingResolution::from(frame);
        if frame.width == 0 || frame.height == 0 || frame.width > 16_384 || frame.height > 16_384 {
            return error_event("keymap frame must be between 1x1 and 16384x16384");
        }
        match CompiledKeymap::from_profile_with_options(
            &profile,
            Some(frame),
            CompileOptions { allow_scripts },
        ) {
            Ok(compiled) => {
                self.compiled = Some(compiled);
                json!({
                    "type": "keymap_status",
                    "payload": {
                        "configured": true,
                        "profile": profile.name,
                        "scripts_enabled": allow_scripts,
                        "active_mapping_ids": [],
                        "active_contacts": self.debug_active_contacts(),
                    }
                })
            }
            Err(error) => error_event(error.to_string()),
        }
    }

    pub(super) fn set_input<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
        keys: Vec<String>,
        pointer_deltas: Vec<BrowserKeymapPointerDelta>,
    ) -> Value {
        let Some(compiled) = self.compiled.as_ref() else {
            return error_event("keymap session is not configured");
        };
        let keys = match normalize_key_state(keys) {
            Ok(keys) => keys,
            Err(error) => {
                self.release(input, orientation);
                return error_event(error.to_string());
            }
        };
        let now = Instant::now();
        let newly_held = keys
            .difference(&self.held)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.held_since.retain(|key, _| keys.contains(key));
        for key in &newly_held {
            self.held_since.insert(key.clone(), now);
        }
        let deltas = pointer_deltas
            .iter()
            .map(|delta| KeymapPointerDelta {
                mapping_id: &delta.mapping_id,
                delta_x: delta.delta_x,
                delta_y: delta.delta_y,
            })
            .collect::<Vec<_>>();
        if let Err(error) = compiled.update_runtime(
            &mut self.runtime,
            &self.held,
            &keys,
            &newly_held,
            &deltas,
            now,
        ) {
            self.release(input, orientation);
            return error_event(error.to_string());
        }
        self.held = keys;
        self.render(input, orientation, now)
    }

    pub(super) fn set_direct_contacts<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
        contacts: Vec<BrowserDirectContact>,
    ) -> Value {
        let Some(contacts) = validate_direct_contacts(contacts) else {
            return error_event(
                "direct touches must use unique identities 0..4 and normalized coordinates",
            );
        };
        self.direct_contacts = contacts;
        self.render(input, orientation, Instant::now())
    }

    pub(super) fn tick<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
    ) -> Option<Value> {
        self.compiled.as_ref()?;
        let previous = self.active_mapping_ids.clone();
        let event = self.render(input, orientation, Instant::now());
        let has_control_mode = event
            .pointer("/payload/control_mode")
            .is_some_and(|value| !value.is_null());
        let has_error = event.pointer("/payload/error").is_some();
        (self.debug_enabled || self.active_mapping_ids != previous || has_control_mode || has_error)
            .then_some(event)
    }

    pub(super) fn stop<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
    ) -> Value {
        self.release(input, orientation);
        json!({
            "type": "keymap_status",
            "payload": {
                "configured": false,
                "active_mapping_ids": [],
                "active_contacts": self.debug_active_contacts(),
            }
        })
    }

    pub(super) fn set_debug_enabled(&mut self, enabled: bool) -> Value {
        self.debug_enabled = enabled;
        self.status_event(None)
    }

    pub(super) fn release<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
    ) {
        if !self.active_contacts.is_empty() {
            let releases = self
                .active_contacts
                .iter()
                .map(|contact| NormalizedTouchContact {
                    touching: false,
                    ..*contact
                })
                .collect::<Vec<_>>();
            send_contacts(input, orientation, &releases);
        }
        for binding in self.active_buttons.iter().rev() {
            input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(
                binding.button,
            )));
        }
        for usage in self.script_device.keyboard.iter().rev().copied() {
            input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(usage)));
        }
        for name in self.script_device.buttons.iter().rev() {
            if let Some(button) = hardware_button(name) {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(button)));
            }
        }
        self.compiled = None;
        self.runtime = KeymapRuntimeState::default();
        self.held.clear();
        self.held_since.clear();
        self.direct_contacts.clear();
        self.active_contacts.clear();
        self.active_buttons.clear();
        self.script_device = ScriptDeviceState::default();
        self.active_mapping_ids.clear();
    }

    fn render<HostPath>(
        &mut self,
        input: &InputSink<HostPath>,
        orientation: Orientation,
        now: Instant,
    ) -> Value {
        let Some(compiled) = self.compiled.as_ref() else {
            return error_event("keymap session is not configured");
        };
        let held_for = self
            .held
            .iter()
            .filter_map(|key| {
                self.held_since
                    .get(key)
                    .map(|at| (key.clone(), now.saturating_duration_since(*at)))
            })
            .collect::<BTreeMap<String, Duration>>();
        let frame = match compiled.frame_with_runtime(&mut self.runtime, &self.held, &held_for, now)
        {
            Ok(frame) => frame,
            Err(error) => {
                self.release(input, orientation);
                return error_event(error.to_string());
            }
        };
        let mut control_mode = None;
        for action in frame.script_actions {
            match action {
                ScriptAction::KeyboardDown { usage } => {
                    if self.script_device.keyboard.insert(usage) {
                        input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardDown(
                            usage,
                        )));
                    }
                }
                ScriptAction::KeyboardUp { usage } => {
                    if self.script_device.keyboard.remove(&usage) {
                        input.send(InputCmd::DeviceInput(DeviceInputCommand::KeyboardUp(usage)));
                    }
                }
                ScriptAction::ButtonDown { name } => {
                    if let Some(button) = hardware_button(&name)
                        && self.script_device.buttons.insert(name)
                    {
                        input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonDown(
                            button,
                        )));
                    }
                }
                ScriptAction::ButtonUp { name } => {
                    if let Some(button) = hardware_button(&name)
                        && self.script_device.buttons.remove(&name)
                    {
                        input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(button)));
                    }
                }
                ScriptAction::Text { text } => {
                    input.send(InputCmd::DeviceInput(DeviceInputCommand::Text(text)));
                }
                ScriptAction::SetRawInput { enabled } => {
                    control_mode = Some(if enabled { "keyboard" } else { "mapping" });
                }
                ScriptAction::Log { message } => {
                    tracing::info!(target: "devicehub_mask::keymap_script", %message);
                }
                ScriptAction::Touch { .. }
                | ScriptAction::EnterFps { .. }
                | ScriptAction::ExitFps
                | ScriptAction::CancelCast { .. }
                | ScriptAction::ReleaseCast => {
                    self.release(input, orientation);
                    return error_event("internal script action escaped the shared keymap runtime");
                }
            }
        }

        let desired_buttons = compiled.active_hardware_buttons(&self.held);
        for binding in self.active_buttons.iter().rev() {
            if !desired_buttons
                .iter()
                .any(|candidate| candidate.name == binding.name)
            {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonUp(
                    binding.button,
                )));
            }
        }
        for binding in &desired_buttons {
            if !self
                .active_buttons
                .iter()
                .any(|candidate| candidate.name == binding.name)
            {
                input.send(InputCmd::DeviceInput(DeviceInputCommand::ButtonDown(
                    binding.button,
                )));
            }
        }
        self.active_buttons = desired_buttons;

        let mut contacts = self.direct_contacts.clone();
        for contact in frame.contacts {
            if contacts.len() >= 5
                || contacts
                    .iter()
                    .any(|candidate| candidate.identity == contact.identity)
            {
                continue;
            }
            contacts.push(contact);
        }
        if contacts != self.active_contacts {
            let report = contacts_with_releases(&contacts, &self.active_contacts);
            send_contacts(input, orientation, &report);
            self.active_contacts = contacts;
        }
        self.active_mapping_ids = frame.active_mapping_ids;
        self.status_event(control_mode)
    }

    fn status_event(&self, control_mode: Option<&str>) -> Value {
        json!({
            "type": "keymap_status",
            "payload": {
                "configured": self.compiled.is_some(),
                "active_mapping_ids": self.active_mapping_ids,
                "active_contact_ids": self.active_contacts.iter().map(|contact| contact.identity).collect::<Vec<_>>(),
                "active_contacts": self.debug_active_contacts(),
                "control_mode": control_mode,
            }
        })
    }

    fn debug_active_contacts(&self) -> Value {
        if !self.debug_enabled {
            return json!([]);
        }
        json!(
            self.active_contacts
                .iter()
                .filter(|contact| contact.touching)
                .map(|contact| {
                    json!({
                        "identity": contact.identity,
                        "touching": contact.touching,
                        "x": contact.x,
                        "y": contact.y,
                    })
                })
                .collect::<Vec<_>>()
        )
    }
}

fn validate_direct_contacts(
    contacts: Vec<BrowserDirectContact>,
) -> Option<Vec<NormalizedTouchContact>> {
    if contacts.len() > 5 {
        return None;
    }
    let mut identities = BTreeSet::new();
    contacts
        .into_iter()
        .map(|contact| {
            if contact.identity >= 5
                || !identities.insert(contact.identity)
                || !contact.x.is_finite()
                || !contact.y.is_finite()
                || !(0.0..=1.0).contains(&contact.x)
                || !(0.0..=1.0).contains(&contact.y)
            {
                return None;
            }
            Some(NormalizedTouchContact {
                identity: contact.identity,
                touching: contact.touching,
                x: contact.x,
                y: contact.y,
            })
        })
        .collect()
}

fn contacts_with_releases(
    current: &[NormalizedTouchContact],
    previous: &[NormalizedTouchContact],
) -> Vec<NormalizedTouchContact> {
    let mut contacts = current.to_vec();
    contacts.extend(previous.iter().filter_map(|contact| {
        (!current
            .iter()
            .any(|candidate| candidate.identity == contact.identity))
        .then_some(NormalizedTouchContact {
            touching: false,
            ..*contact
        })
    }));
    contacts
}

fn send_contacts<HostPath>(
    input: &InputSink<HostPath>,
    orientation: Orientation,
    contacts: &[NormalizedTouchContact],
) {
    let turns = orientation.quarter_turns_cw();
    input.send(InputCmd::DeviceInput(DeviceInputCommand::MultiTouchFrame(
        contacts
            .iter()
            .map(|contact| {
                let (x, y) = unrotate_norm(contact.x, contact.y, turns);
                TouchContact {
                    identity: contact.identity,
                    touching: contact.touching,
                    x: norm(x),
                    y: norm(y),
                }
            })
            .collect(),
    )));
}

fn error_event(message: impl ToString) -> Value {
    json!({
        "type": "keymap_status",
        "payload": {
            "configured": false,
            "active_mapping_ids": [],
            "active_contacts": [],
            "error": message.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devicehub_core::default_hardware_bindings;
    use serde_json::json;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn browser_session_uses_shared_script_runtime_and_releases_contacts() {
        let input = InputSink::<std::path::PathBuf>::default();
        let (sender, mut receiver) = unbounded_channel();
        input.set(Some(sender));
        let mut session = BrowserKeymapSession::default();
        let profile = KeyMappingProfile {
            version: 2,
            name: "desktop".into(),
            mappings: vec![json!({
                "id": "script", "type": "Script", "position": { "x": 0.5, "y": 0.5 },
                "bind": ["KeyF"], "interval": 100,
                "pressed_script": "tap(1, 250, 250)", "held_script": "", "released_script": ""
            })],
            bundle_identifiers: Vec::new(),
            target_resolution: None,
            hardware_bindings: default_hardware_bindings(),
        };
        session.configure(
            &input,
            Orientation::Portrait,
            profile,
            BrowserKeymapResolution {
                width: 1000,
                height: 500,
            },
            true,
        );
        let debug = session.set_debug_enabled(true);
        assert_eq!(debug["payload"]["active_contacts"], json!([]));
        let status = session.set_input(
            &input,
            Orientation::Portrait,
            vec!["KeyF".into()],
            Vec::new(),
        );
        assert_eq!(status["payload"]["active_contacts"][0]["identity"], 1);
        let InputCmd::DeviceInput(DeviceInputCommand::MultiTouchFrame(down)) =
            receiver.try_recv().unwrap()
        else {
            panic!("script must emit a touch frame");
        };
        assert!(down[0].touching);
        session.release(&input, Orientation::Portrait);
        let InputCmd::DeviceInput(DeviceInputCommand::MultiTouchFrame(up)) =
            receiver.try_recv().unwrap()
        else {
            panic!("release must emit a touch frame");
        };
        assert!(!up[0].touching);
    }
}
