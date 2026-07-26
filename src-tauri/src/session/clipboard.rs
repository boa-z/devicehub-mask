//! Desktop `arboard` adapter for the runtime-owned Pasteboard session.

use std::borrow::Cow;

use devicehub_runtime::{ClipboardImage, HostClipboard};

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ArboardClipboardProvider;

impl devicehub_runtime::HostClipboardProvider for ArboardClipboardProvider {
    fn connect(&self) -> Result<Box<dyn HostClipboard>, String> {
        connect_host()
    }
}

pub(super) fn connect_host() -> Result<Box<dyn HostClipboard>, String> {
    arboard::Clipboard::new()
        .map(|clipboard| Box::new(ArboardClipboard(clipboard)) as Box<dyn HostClipboard>)
        .map_err(|error| format!("unable to open host clipboard: {error}"))
}

struct ArboardClipboard(arboard::Clipboard);

impl HostClipboard for ArboardClipboard {
    fn get_text(&mut self) -> Result<String, String> {
        self.0
            .get_text()
            .map_err(|error| format!("unable to read host clipboard text: {error}"))
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        self.0
            .set_text(text)
            .map_err(|error| format!("unable to write host clipboard text: {error}"))
    }

    fn get_image(&mut self) -> Result<ClipboardImage, String> {
        self.0
            .get_image()
            .map(|image| ClipboardImage {
                width: image.width,
                height: image.height,
                bytes: image.bytes.into_owned(),
            })
            .map_err(|error| format!("unable to read host clipboard image: {error}"))
    }

    fn set_image(&mut self, image: ClipboardImage) -> Result<(), String> {
        self.0
            .set_image(arboard::ImageData {
                width: image.width,
                height: image.height,
                bytes: Cow::Owned(image.bytes),
            })
            .map_err(|error| format!("unable to write host clipboard image: {error}"))
    }
}
