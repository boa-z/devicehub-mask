//! Resolution-independent input and orientation models.

/// HID definition for one hardware control exposed by every input adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareButton {
    pub usage_page: u64,
    pub usage_code: u64,
    pub hold_ms: u64,
}

const HARDWARE_BUTTONS: [(&str, HardwareButton); 7] = [
    (
        "home",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0x40,
            hold_ms: 80,
        },
    ),
    (
        "lock",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0x30,
            hold_ms: 200,
        },
    ),
    (
        "volume-up",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0xE9,
            hold_ms: 80,
        },
    ),
    (
        "volume-down",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0xEA,
            hold_ms: 80,
        },
    ),
    (
        "mute",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0xE2,
            hold_ms: 80,
        },
    ),
    (
        "siri",
        HardwareButton {
            usage_page: 0x0C,
            usage_code: 0xCF,
            hold_ms: 1200,
        },
    ),
    (
        "action",
        HardwareButton {
            usage_page: 0x0B,
            usage_code: 0x2D,
            hold_ms: 80,
        },
    ),
];

/// Hardware controls accepted by every input adapter and mapping profile.
pub const HARDWARE_BUTTON_NAMES: [&str; 7] = [
    HARDWARE_BUTTONS[0].0,
    HARDWARE_BUTTONS[1].0,
    HARDWARE_BUTTONS[2].0,
    HARDWARE_BUTTONS[3].0,
    HARDWARE_BUTTONS[4].0,
    HARDWARE_BUTTONS[5].0,
    HARDWARE_BUTTONS[6].0,
];

pub fn hardware_button(name: &str) -> Option<HardwareButton> {
    HARDWARE_BUTTONS
        .iter()
        .find_map(|(candidate, button)| (*candidate == name).then_some(*button))
}

/// Which way to rotate the device by 90 degrees.
#[derive(Debug, Clone, Copy)]
pub enum RotateDir {
    Left,
    Right,
}

/// The device's screen orientation reported by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

impl Orientation {
    /// Quarter-turns clockwise to show the native-portrait frame upright.
    pub fn quarter_turns_cw(self) -> u8 {
        match self {
            Orientation::Portrait => 0,
            Orientation::LandscapeRight => 1,
            Orientation::PortraitUpsideDown => 2,
            Orientation::LandscapeLeft => 3,
        }
    }
}

/// Keyboard modifiers held while dispatching a key combination.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyMods {
    pub cmd: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Modifier usages in press order; callers release them in reverse order.
pub fn modifier_key_usages(modifiers: KeyMods) -> [(u64, bool); 4] {
    [
        (0xE0, modifiers.ctrl),
        (0xE2, modifiers.alt),
        (0xE3, modifiers.cmd),
        (0xE1, modifiers.shift),
    ]
}

/// Map an ASCII character to its HID Keyboard/Keypad usage and Shift state.
pub fn ascii_key_usage(character: char) -> Option<(u64, bool)> {
    Some(match character {
        'a'..='z' => (0x04 + (character as u64 - 'a' as u64), false),
        'A'..='Z' => (0x04 + (character as u64 - 'A' as u64), true),
        '1'..='9' => (0x1E + (character as u64 - '1' as u64), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        '-' => (0x2D, false),
        '_' => (0x2D, true),
        '=' => (0x2E, false),
        '+' => (0x2E, true),
        '[' => (0x2F, false),
        '{' => (0x2F, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        _ => return None,
    })
}

/// Clamp a `0.0..=1.0` fraction to a normalized `0..=65535` touch coordinate.
pub fn norm(frac: f32) -> u16 {
    (frac.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Map an upright display point back into native framebuffer coordinates.
pub fn unrotate_norm(dx: f32, dy: f32, turns: u8) -> (f32, f32) {
    match turns % 4 {
        0 => (dx, dy),
        1 => (dy, 1.0 - dx),
        2 => (1.0 - dx, 1.0 - dy),
        _ => (1.0 - dy, dx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_button_names_resolve_to_stable_hid_definitions() {
        for name in HARDWARE_BUTTON_NAMES {
            assert!(hardware_button(name).is_some(), "missing {name}");
        }
        assert_eq!(hardware_button("home").unwrap().usage_code, 0x40);
        assert_eq!(hardware_button("action").unwrap().usage_page, 0x0B);
        assert!(hardware_button("unknown").is_none());
    }

    #[test]
    fn ascii_keys_preserve_us_layout_shift_semantics() {
        assert_eq!(ascii_key_usage('a'), Some((0x04, false)));
        assert_eq!(ascii_key_usage('A'), Some((0x04, true)));
        assert_eq!(ascii_key_usage('!'), Some((0x1E, true)));
        assert_eq!(ascii_key_usage('\n'), Some((0x28, false)));
        assert_eq!(ascii_key_usage('中'), None);
    }

    #[test]
    fn modifiers_are_ordered_for_balanced_hid_chords() {
        let usages = modifier_key_usages(KeyMods {
            cmd: true,
            shift: true,
            ctrl: false,
            alt: false,
        });
        assert_eq!(
            usages,
            [(0xE0, false), (0xE2, false), (0xE3, true), (0xE1, true)]
        );
    }
}
