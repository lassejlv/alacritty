use log::{debug, warn};
use winit::raw_window_handle::RawDisplayHandle;

use alacritty_terminal::clipboard::{
    TerminalClipboardContent, TerminalClipboardLocation, TerminalClipboardReadRequest,
    TerminalClipboardReadResult, TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
};
use alacritty_terminal::term::ClipboardType;

pub mod kitty;
#[cfg(target_os = "macos")]
use crate::platform::macos::clipboard as macos;

#[cfg(any(feature = "x11", target_os = "macos", windows))]
use copypasta::ClipboardContext;
use copypasta::ClipboardProvider;
use copypasta::nop_clipboard::NopClipboardContext;
#[cfg(all(feature = "wayland", not(any(target_os = "macos", windows))))]
use copypasta::wayland_clipboard;
#[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
use copypasta::x11_clipboard::{Primary as X11SelectionClipboard, X11ClipboardContext};

pub struct Clipboard {
    clipboard: Box<dyn ClipboardProvider>,
    selection: Option<Box<dyn ClipboardProvider>>,
    #[cfg(target_os = "macos")]
    native: Option<macos::MimeClipboard>,
}

impl Clipboard {
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn with_native(native: macos::MimeClipboard) -> Self {
        Self { native: Some(native), ..Self::new_nop() }
    }

    pub unsafe fn new(display: RawDisplayHandle) -> Self {
        match display {
            #[cfg(all(feature = "wayland", not(any(target_os = "macos", windows))))]
            RawDisplayHandle::Wayland(display) => {
                let (selection, clipboard) = unsafe {
                    wayland_clipboard::create_clipboards_from_external(display.display.as_ptr())
                };
                Self { clipboard: Box::new(clipboard), selection: Some(Box::new(selection)) }
            },
            _ => Self::default(),
        }
    }

    /// Used for tests, to handle missing clipboard provider when built without the `x11`
    /// feature, and as default clipboard value.
    pub fn new_nop() -> Self {
        Self {
            clipboard: Box::new(NopClipboardContext::new().unwrap()),
            selection: None,
            #[cfg(target_os = "macos")]
            native: None,
        }
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", windows))]
        return Self {
            clipboard: Box::new(ClipboardContext::new().unwrap()),
            selection: None,
            #[cfg(target_os = "macos")]
            native: Some(macos::MimeClipboard::new()),
        };

        #[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
        return Self {
            clipboard: Box::new(ClipboardContext::new().unwrap()),
            selection: Some(Box::new(X11ClipboardContext::<X11SelectionClipboard>::new().unwrap())),
        };

        #[cfg(not(any(feature = "x11", target_os = "macos", windows)))]
        return Self::new_nop();
    }
}

impl Clipboard {
    pub fn read_protocol(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            return native.read(request);
        }
        let ty = match request.location {
            TerminalClipboardLocation::Primary if self.selection.is_none() => {
                return TerminalClipboardReadResult::Unsupported;
            },
            TerminalClipboardLocation::Primary => ClipboardType::Selection,
            TerminalClipboardLocation::Clipboard => ClipboardType::Clipboard,
        };
        let mut contents = Vec::new();
        if request.mime_types.iter().any(|mime| mime == "text/plain") {
            let data = self.load(ty).into_bytes();
            if data.len() > alacritty_terminal::clipboard::MAX_WRITE_BYTES {
                return TerminalClipboardReadResult::Busy;
            }
            contents.push(TerminalClipboardContent { mime_type: "text/plain".into(), data });
        }
        TerminalClipboardReadResult::Success {
            available_formats: vec!["text/plain".into()],
            contents,
            remember_permission: false,
        }
    }

    pub fn write_protocol(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            return native.write(request);
        }
        let ty = match request.location {
            TerminalClipboardLocation::Primary if self.selection.is_none() => {
                return TerminalClipboardWriteResult::Unsupported;
            },
            TerminalClipboardLocation::Primary => ClipboardType::Selection,
            TerminalClipboardLocation::Clipboard => ClipboardType::Clipboard,
        };
        if request.contents.iter().any(|entry| entry.mime_type != "text/plain") {
            return TerminalClipboardWriteResult::Unsupported;
        }
        let data = request.contents.into_iter().next().map_or(Vec::new(), |entry| entry.data);
        let Ok(text) = String::from_utf8(data) else {
            return TerminalClipboardWriteResult::InvalidData;
        };
        let provider = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            _ => &mut self.clipboard,
        };
        match provider.set_contents(text) {
            Ok(()) => TerminalClipboardWriteResult::Success { remember_permission: false },
            Err(_) => TerminalClipboardWriteResult::IoError,
        }
    }

    pub fn protocol_formats(&mut self, location: TerminalClipboardLocation) -> Vec<String> {
        let request = TerminalClipboardReadRequest {
            location,
            mime_types: Vec::new(),
            list_available: true,
            name: None,
            permission_granted: true,
            can_remember_permission: false,
        };
        match self.read_protocol(request) {
            TerminalClipboardReadResult::Success { available_formats, .. } => available_formats,
            _ => Vec::new(),
        }
    }

    pub fn store(&mut self, ty: ClipboardType, text: impl Into<String>) {
        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            (ClipboardType::Selection, None) => return,
            _ => &mut self.clipboard,
        };

        clipboard.set_contents(text.into()).unwrap_or_else(|err| {
            warn!("Unable to store text in clipboard: {err}");
        });
    }

    pub fn load(&mut self, ty: ClipboardType) -> String {
        let clipboard = match (ty, &mut self.selection) {
            (ClipboardType::Selection, Some(provider)) => provider,
            _ => &mut self.clipboard,
        };

        match clipboard.get_contents() {
            Err(err) => {
                debug!("Unable to load text from clipboard: {err}");
                String::new()
            },
            Ok(text) => text,
        }
    }
}
