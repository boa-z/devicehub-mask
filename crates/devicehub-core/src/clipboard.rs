//! Clipboard event metadata and bounded text policies.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardContentKind {
    Text,
    Image,
}

/// A transient clipboard sync event containing no clipboard payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClipboardEvent {
    /// `true` if the content came from the device, `false` for host to device.
    pub from_device: bool,
    pub kind: ClipboardContentKind,
    pub preview: String,
}

/// Single-line clipboard preview: collapse whitespace and truncate to `max` chars.
pub fn clipboard_preview(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        let mut preview: String = collapsed.chars().take(max).collect();
        preview.push_str("...");
        preview
    } else {
        collapsed
    }
}

pub fn validate_paste_text(text: &str) -> Result<usize, &'static str> {
    let characters = text.chars().count();
    if text.is_empty() || text.len() > 4_096 || characters > 1_024 || text.contains('\0') {
        Err(
            "paste text must contain 1..1024 characters, fit in 4096 UTF-8 bytes, and contain no NUL bytes",
        )
    } else {
        Ok(characters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_preview_and_paste_validation_are_bounded() {
        assert_eq!(clipboard_preview(" a\n b  c ", 4), "a b ...");
        assert_eq!(validate_paste_text("你好, iPhone").unwrap(), 10);
        for invalid in [String::new(), "bad\0text".into(), "x".repeat(1_025)] {
            assert!(validate_paste_text(&invalid).is_err());
        }
        assert!(validate_paste_text(&"界".repeat(1_024)).is_ok());
        assert!(validate_paste_text(&"😀".repeat(1_024)).is_ok());
    }
}
