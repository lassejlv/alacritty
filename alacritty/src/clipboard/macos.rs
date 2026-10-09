//! Map MIME types to native pasteboard types without reducing binary data to strings.
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
use objc2_foundation::{NSArray, NSData, NSString};
use objc2_uniform_type_identifiers::UTType;
use std::fmt::Write as _;

use alacritty_terminal::clipboard::{
    MAX_MIME_TYPES, MAX_WRITE_BYTES, TerminalClipboardContent, TerminalClipboardLocation,
    TerminalClipboardReadRequest, TerminalClipboardReadResult, TerminalClipboardWriteRequest,
    TerminalClipboardWriteResult,
};

pub struct MimeClipboard {
    board: Retained<NSPasteboard>,
}

impl MimeClipboard {
    pub fn new() -> Self {
        Self { board: NSPasteboard::generalPasteboard() }
    }

    fn formats(&self) -> Vec<String> {
        let Some(types) = self.board.types() else { return Vec::new() };
        let mut formats = Vec::new();
        // Exact MIME names preserve aliases even when macOS canonicalizes their UTI.
        for index in 0..types.len() {
            let text = types.objectAtIndex(index).to_string();
            if let Some(mime) = exact_mime(&text) {
                if !formats.contains(&mime) {
                    formats.push(mime);
                }
            }
            if formats.len() >= MAX_MIME_TYPES {
                return formats;
            }
        }
        for index in 0..types.len() {
            let ty = types.objectAtIndex(index);
            let text = ty.to_string();
            let mime = if text == "public.utf8-plain-text" {
                Some("text/plain".into())
            } else if text == "public.data" {
                Some("application/octet-stream".into())
            } else if text.contains('/') {
                Some(text)
            } else {
                UTType::typeWithIdentifier(&ty)
                    .and_then(|ty| ty.preferredMIMEType())
                    .map(|mime| mime.to_string())
            };
            if let Some(mime) = mime {
                if !formats.contains(&mime) {
                    formats.push(mime);
                }
            }
            if formats.len() >= MAX_MIME_TYPES {
                break;
            }
        }
        formats
    }

    pub fn read(&self, request: TerminalClipboardReadRequest) -> TerminalClipboardReadResult {
        if request.location == TerminalClipboardLocation::Primary {
            return TerminalClipboardReadResult::Unsupported;
        }
        let before = self.board.changeCount();
        let formats = self.formats();
        let mut contents = Vec::new();
        let mut total = 0usize;
        for mime in request.mime_types {
            let Some(data) = self
                .board
                .dataForType(&exact_type(&mime))
                .or_else(|| self.board.dataForType(&native_type(&mime)))
            else {
                continue;
            };
            total = total.saturating_add(data.len());
            if total > MAX_WRITE_BYTES {
                return TerminalClipboardReadResult::Busy;
            }
            contents.push(TerminalClipboardContent { mime_type: mime, data: data.to_vec() });
        }
        if self.board.changeCount() != before {
            return TerminalClipboardReadResult::Busy;
        }
        TerminalClipboardReadResult::Success {
            available_formats: formats,
            contents,
            remember_permission: false,
        }
    }

    pub fn write(&self, request: TerminalClipboardWriteRequest) -> TerminalClipboardWriteResult {
        if request.location == TerminalClipboardLocation::Primary {
            return TerminalClipboardWriteResult::Unsupported;
        }
        // Assemble all representations before replacing the clipboard.
        let item = NSPasteboardItem::new();
        let mut types = Vec::new();
        for content in &request.contents {
            let data = NSData::with_bytes(&content.data);
            for ty in [exact_type(&content.mime_type), native_type(&content.mime_type)] {
                if types.contains(&ty) {
                    continue;
                }
                if !item.setData_forType(&data, &ty) {
                    return TerminalClipboardWriteResult::IoError;
                }
                types.push(ty);
            }
        }
        self.board.clearContents();
        if !request.contents.is_empty() {
            let objects = NSArray::from_slice(&[ProtocolObject::from_ref(&*item)]);
            if !self.board.writeObjects(&objects) {
                return TerminalClipboardWriteResult::IoError;
            }
        }
        TerminalClipboardWriteResult::Success { remember_permission: false }
    }
}

fn native_type(mime: &str) -> Retained<NSString> {
    if mime == "text/plain" {
        return NSString::from_str("public.utf8-plain-text");
    }
    UTType::typeWithMIMEType(&NSString::from_str(mime))
        .map_or_else(|| exact_type(mime), |ty| ty.identifier())
}

const MIME_PREFIX: &str = "org.alacritty.kitty.mime.";

fn exact_type(mime: &str) -> Retained<NSString> {
    let mut identifier = String::with_capacity(MIME_PREFIX.len() + mime.len() * 2);
    identifier.push_str(MIME_PREFIX);
    for byte in mime.bytes() {
        write!(&mut identifier, "{byte:02x}").expect("String formatting is infallible");
    }
    NSString::from_str(&identifier)
}

fn exact_mime(ty: &str) -> Option<String> {
    let encoded = ty.strip_prefix(MIME_PREFIX)?;
    if encoded.len() % 2 != 0 || encoded.len() > 512 {
        return None;
    }
    let bytes: Option<Vec<_>> = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ClearPrivateBoard(Retained<NSPasteboard>);
    impl Drop for ClearPrivateBoard {
        fn drop(&mut self) {
            self.0.clearContents();
        }
    }

    #[test]
    #[ignore = "requires the official kitten client path in ALACRITTY_TEST_KITTEN"]
    fn official_kitten_clipboard_round_trip_on_a_private_pasteboard() {
        use alacritty_terminal::clipboard::KittyClipboardHostState;
        use alacritty_terminal::event::{Event, EventListener};
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::term::{Config, Osc52, Term};
        use alacritty_terminal::vte::ansi::Processor;
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Events(Arc<Mutex<Vec<Event>>>);
        impl EventListener for Events {
            fn send_event(&self, event: Event) {
                self.0.lock().unwrap().push(event);
            }
        }
        struct Size;
        impl Dimensions for Size {
            fn total_lines(&self) -> usize {
                25
            }

            fn screen_lines(&self) -> usize {
                25
            }

            fn columns(&self) -> usize {
                100
            }
        }

        let kitten = std::env::var("ALACRITTY_TEST_KITTEN").expect("set the official client path");
        let directory = tempfile::tempdir().unwrap();
        let driver = directory.path().join("kitten_pty.py");
        std::fs::write(&driver, include_str!("tests/kitten_pty.py")).unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        let text = directory.path().join("text");
        let data: Vec<_> = (0..20_000).map(|index| (index % 256) as u8).collect();
        std::fs::write(&input, &data).unwrap();
        std::fs::write(&text, "Kitty clipboard ø\n").unwrap();
        let name =
            NSString::from_str(&format!("org.alacritty.kitty-client.{}", std::process::id()));
        let board = NSPasteboard::pasteboardWithName(&name);
        let _cleanup = ClearPrivateBoard(board.clone());
        let mut clipboard = crate::clipboard::Clipboard::new_nop();
        clipboard.native = Some(MimeClipboard { board: board.clone() });
        let mut host = crate::clipboard::kitty::Host {
            clipboard: &mut clipboard,
            permission: Osc52::CopyPaste,
            focused: true,
        };
        let mut state = KittyClipboardHostState::new();
        let mut format_output = Vec::new();

        for arguments in [
            vec![
                "--mime",
                "application/octet-stream",
                "--mime",
                "text/plain",
                "--alias",
                "application/x-client=application/octet-stream",
                input.to_str().unwrap(),
                text.to_str().unwrap(),
            ],
            vec!["--get-clipboard", "--mime", "application/octet-stream", output.to_str().unwrap()],
            vec!["--get-clipboard", "--mime", ".", "/dev/stdout"],
        ] {
            let listing = arguments.contains(&".");
            let events = Events::default();
            let mut terminal = Term::new(Config::default(), &Size, events.clone());
            let mut parser: Processor = Processor::new();
            let mut child = Command::new("/usr/bin/python3")
                .arg(&driver)
                .arg(&kitten)
                .arg("clipboard")
                .args(arguments)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            let mut exited = false;
            let mut transcript = Vec::new();
            for line in BufReader::new(child.stdout.take().unwrap()).lines() {
                let message: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
                if let Some(exit) = message.get("exit") {
                    let packets: Vec<_> = transcript
                        .split(|b| *b == 27)
                        .filter(|p| p.starts_with(b"]5522;"))
                        .map(|p| {
                            let mut fields = p.split(|b| *b == b';');
                            fields.next();
                            (
                                String::from_utf8_lossy(fields.next().unwrap_or_default())
                                    .into_owned(),
                                fields.next().map_or(0, |payload| payload.len()),
                            )
                        })
                        .collect();
                    assert_eq!(exit.as_i64(), Some(0), "clipboard packets: {packets:?}");
                    exited = true;
                    break;
                }
                let bytes = BASE64.decode(message["data"].as_str().unwrap()).unwrap();
                transcript.extend_from_slice(&bytes);
                parser.advance(&mut terminal, &bytes);
                let mut replies = Vec::new();
                for event in events.0.lock().unwrap().drain(..) {
                    match event {
                        Event::KittyClipboard(osc) => {
                            replies.extend(state.handle_osc(osc, &mut host))
                        },
                        Event::PtyWrite(bytes) => replies.push(bytes.into_bytes()),
                        _ => (),
                    }
                }
                for reply in replies {
                    writeln!(input, "{}", serde_json::json!({"reply": BASE64.encode(reply)}))
                        .unwrap();
                    input.flush().unwrap();
                }
            }
            assert!(child.wait().unwrap().success());
            assert!(exited);
            if listing {
                format_output = transcript;
            }
        }
        assert_eq!(std::fs::read(output).unwrap(), data);
        let available = String::from_utf8_lossy(&format_output);
        assert!(available.contains("application/octet-stream"));
        assert!(available.contains("application/x-client"));
        assert!(available.contains("text/plain"));
        board.clearContents();
    }

    #[test]
    fn native_mime_data_round_trips_on_a_private_pasteboard() {
        let name = NSString::from_str(&format!("org.alacritty.kitty-test.{}", std::process::id()));
        let clipboard = MimeClipboard { board: NSPasteboard::pasteboardWithName(&name) };
        let _cleanup = ClearPrivateBoard(clipboard.board.clone());
        let entries = vec![
            TerminalClipboardContent {
                mime_type: "text/plain".into(),
                data: "clipboard ø".as_bytes().to_vec(),
            },
            TerminalClipboardContent {
                mime_type: "text/html".into(),
                data: b"<b>clipboard</b>".to_vec(),
            },
            TerminalClipboardContent {
                mime_type: "image/png".into(),
                data: vec![0, 255, 13, 10, 1],
            },
            TerminalClipboardContent {
                mime_type: "application/x-alacritty-test".into(),
                data: vec![0, 1, 255],
            },
        ];
        assert!(matches!(
            clipboard.write(TerminalClipboardWriteRequest {
                location: TerminalClipboardLocation::Clipboard,
                contents: entries.clone(),
                name: None,
                permission_granted: true,
                can_remember_permission: false,
            }),
            TerminalClipboardWriteResult::Success { .. }
        ));
        let TerminalClipboardReadResult::Success { available_formats, contents, .. } = clipboard
            .read(TerminalClipboardReadRequest {
                location: TerminalClipboardLocation::Clipboard,
                mime_types: entries.iter().map(|e| e.mime_type.clone()).collect(),
                list_available: true,
                name: None,
                permission_granted: true,
                can_remember_permission: false,
            })
        else {
            panic!("native read failed")
        };
        assert_eq!(contents, entries);
        assert!(available_formats.contains(&"text/plain".into()));
        assert!(available_formats.contains(&"image/png".into()));
        clipboard.board.clearContents();
    }
}
