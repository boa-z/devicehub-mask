//! Execution of validated input commands against CoreDevice HID services.

use devicehub_core::{
    DeviceInputCommand, HardwareButton, KeyMods, Orientation, OrientationSlot, RotateDir,
    ascii_key_usage, modifier_key_usages,
};
use idevice::{
    IdeviceError, ReadWrite,
    core_device::{
        Orientation as DeviceOrientation, OrientationServiceClient, RotationDirection,
        hid::{
            ButtonState, IndigoHidClient, TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE,
            UniversalHidServiceClient,
        },
    },
};

use super::hid::touchscreen_contacts;

/// Owns the HID services used to execute input for one connected device session.
pub(crate) struct DeviceInputDispatcher {
    touch: UniversalHidServiceClient<Box<dyn ReadWrite>>,
    keyboard: IndigoHidClient<Box<dyn ReadWrite>>,
    orientation: Option<OrientationServiceClient<Box<dyn ReadWrite>>>,
    orientation_view: OrientationSlot,
}

impl DeviceInputDispatcher {
    pub(crate) fn new(
        touch: UniversalHidServiceClient<Box<dyn ReadWrite>>,
        keyboard: IndigoHidClient<Box<dyn ReadWrite>>,
        orientation: Option<OrientationServiceClient<Box<dyn ReadWrite>>>,
        orientation_view: OrientationSlot,
    ) -> Self {
        Self {
            touch,
            keyboard,
            orientation,
            orientation_view,
        }
    }

    pub(crate) async fn dispatch(
        &mut self,
        command: DeviceInputCommand,
    ) -> Result<(), IdeviceError> {
        match command {
            DeviceInputCommand::Tap { x, y } => self.touch.tap(x, y).await,
            DeviceInputCommand::TouchDown { x, y } | DeviceInputCommand::TouchMove { x, y } => {
                self.touch
                    .send_touchscreen(TOUCHSCREEN_STATE_CONTACT, x, y, None)
                    .await
            }
            DeviceInputCommand::TouchUp { x, y } => {
                self.touch
                    .send_touchscreen(TOUCHSCREEN_STATE_RELEASE, x, y, None)
                    .await
            }
            DeviceInputCommand::MultiTouchFrame(contacts) => {
                let contacts = touchscreen_contacts(&contacts);
                self.touch.send_multitouch(&contacts, None).await
            }
            DeviceInputCommand::Text(text) => {
                for character in text.chars() {
                    if let Some((usage, shift)) = ascii_key_usage(character) {
                        self.type_key(
                            usage,
                            KeyMods {
                                shift,
                                ..KeyMods::default()
                            },
                        )
                        .await?;
                    }
                }
                Ok(())
            }
            DeviceInputCommand::KeyUsage(usage) => self.type_key(usage, KeyMods::default()).await,
            DeviceInputCommand::KeyCombo { usage, mods } => self.type_key(usage, mods).await,
            DeviceInputCommand::KeyboardDown(usage) => {
                self.keyboard.send_keyboard(usage, ButtonState::Down).await
            }
            DeviceInputCommand::KeyboardUp(usage) => {
                self.keyboard.send_keyboard(usage, ButtonState::Up).await
            }
            DeviceInputCommand::Button(button) => {
                self.send_button(button, ButtonState::Down).await?;
                tokio::time::sleep(std::time::Duration::from_millis(button.hold_ms)).await;
                self.send_button(button, ButtonState::Up).await
            }
            DeviceInputCommand::ButtonDown(button) => {
                self.send_button(button, ButtonState::Down).await
            }
            DeviceInputCommand::ButtonUp(button) => self.send_button(button, ButtonState::Up).await,
            DeviceInputCommand::Rotate(direction) => self.rotate(direction).await,
        }
    }

    async fn send_button(
        &mut self,
        button: HardwareButton,
        state: ButtonState,
    ) -> Result<(), IdeviceError> {
        self.keyboard
            .send_button(button.usage_page, button.usage_code, state)
            .await
    }

    async fn type_key(&mut self, usage: u64, mods: KeyMods) -> Result<(), IdeviceError> {
        let modifiers = modifier_key_usages(mods);
        for (modifier, held) in modifiers {
            if held {
                self.keyboard
                    .send_keyboard(modifier, ButtonState::Down)
                    .await?;
            }
        }
        self.keyboard
            .send_keyboard(usage, ButtonState::Down)
            .await?;
        self.keyboard.send_keyboard(usage, ButtonState::Up).await?;
        for (modifier, held) in modifiers.iter().rev() {
            if *held {
                self.keyboard
                    .send_keyboard(*modifier, ButtonState::Up)
                    .await?;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        Ok(())
    }

    async fn rotate(&mut self, direction: RotateDir) -> Result<(), IdeviceError> {
        let Some(client) = &mut self.orientation else {
            tracing::warn!("rotate requested but orientation service unavailable");
            return Ok(());
        };
        let device_direction = match direction {
            RotateDir::Left => RotationDirection::Left,
            RotateDir::Right => RotationDirection::Right,
        };
        let state = client.rotate(device_direction).await?;
        tracing::info!(
            "rotated {direction:?} -> {:?} (non-flat {:?})",
            state.orientation,
            state.non_flat_orientation,
        );
        if let Some(orientation) = display_orientation(state.non_flat_orientation) {
            self.orientation_view.set(orientation);
        }
        Ok(())
    }
}

fn display_orientation(orientation: DeviceOrientation) -> Option<Orientation> {
    match orientation {
        DeviceOrientation::Portrait => Some(Orientation::Portrait),
        DeviceOrientation::PortraitUpsideDown => Some(Orientation::PortraitUpsideDown),
        DeviceOrientation::LandscapeLeft => Some(Orientation::LandscapeLeft),
        DeviceOrientation::LandscapeRight => Some(Orientation::LandscapeRight),
        DeviceOrientation::FaceUp | DeviceOrientation::FaceDown | DeviceOrientation::Unknown(_) => {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_ignores_flat_or_unknown_device_orientation() {
        assert_eq!(display_orientation(DeviceOrientation::FaceUp), None);
        assert_eq!(display_orientation(DeviceOrientation::FaceDown), None);
        assert_eq!(
            display_orientation(DeviceOrientation::Unknown("future".into())),
            None
        );
    }

    #[test]
    fn rotation_preserves_non_flat_device_orientation() {
        assert_eq!(
            display_orientation(DeviceOrientation::LandscapeLeft),
            Some(Orientation::LandscapeLeft)
        );
        assert_eq!(
            display_orientation(DeviceOrientation::PortraitUpsideDown),
            Some(Orientation::PortraitUpsideDown)
        );
    }
}
