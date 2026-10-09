//! Permissions for terminal-originated clipboard requests. Native access stays on the UI thread.
use alacritty_terminal::clipboard::{
    ClipboardHost, TerminalClipboardReadRequest, TerminalClipboardReadResult,
    TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
};
use alacritty_terminal::term::Osc52;

use super::Clipboard;

pub struct Host<'a> {
    pub clipboard: &'a mut Clipboard,
    pub permission: Osc52,
    pub focused: bool,
}

impl ClipboardHost for Host<'_> {
    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        let allowed = request.mime_types.is_empty()
            || request.permission_granted
            || matches!(self.permission, Osc52::OnlyPaste | Osc52::CopyPaste);
        if !self.focused || self.permission == Osc52::Disabled || !allowed {
            return TerminalClipboardReadResult::Denied;
        }
        self.clipboard.read_protocol(request)
    }

    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        if !self.focused || !matches!(self.permission, Osc52::OnlyCopy | Osc52::CopyPaste) {
            return TerminalClipboardWriteResult::Denied;
        }
        self.clipboard.write_protocol(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::clipboard::TerminalClipboardLocation;

    fn request(data: bool, grant: bool) -> TerminalClipboardReadRequest {
        TerminalClipboardReadRequest {
            location: TerminalClipboardLocation::Clipboard,
            mime_types: if data { vec!["text/plain".into()] } else { Vec::new() },
            list_available: !data,
            name: None,
            permission_granted: grant,
            can_remember_permission: false,
        }
    }

    #[test]
    fn default_policy_allows_discovery_and_explicit_paste_but_denies_unsolicited_reads() {
        let mut clipboard = Clipboard::new_nop();
        let mut host =
            Host { clipboard: &mut clipboard, permission: Osc52::OnlyCopy, focused: true };
        assert!(matches!(
            host.read_clipboard(request(false, false)),
            TerminalClipboardReadResult::Success { .. }
        ));
        assert_eq!(host.read_clipboard(request(true, false)), TerminalClipboardReadResult::Denied);
        assert!(matches!(
            host.read_clipboard(request(true, true)),
            TerminalClipboardReadResult::Success { .. }
        ));
        host.permission = Osc52::Disabled;
        assert_eq!(host.read_clipboard(request(true, true)), TerminalClipboardReadResult::Denied);
        assert_eq!(host.read_clipboard(request(false, true)), TerminalClipboardReadResult::Denied);
    }

    #[test]
    fn focus_and_config_gate_every_native_request_even_with_a_password_grant() {
        let mut clipboard = Clipboard::new_nop();
        let mut host =
            Host { clipboard: &mut clipboard, permission: Osc52::CopyPaste, focused: false };
        assert_eq!(host.read_clipboard(request(true, true)), TerminalClipboardReadResult::Denied);
        let write = TerminalClipboardWriteRequest {
            location: TerminalClipboardLocation::Clipboard,
            contents: Vec::new(),
            name: None,
            permission_granted: true,
            can_remember_permission: true,
        };
        assert_eq!(host.write_clipboard(write.clone()), TerminalClipboardWriteResult::Denied);
        host.focused = true;
        host.permission = Osc52::OnlyPaste;
        assert_eq!(host.write_clipboard(write), TerminalClipboardWriteResult::Denied);
        assert!(matches!(
            host.read_clipboard(request(true, false)),
            TerminalClipboardReadResult::Success { .. }
        ));
    }
}
