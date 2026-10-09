# Kitty clipboard protocol

Alacritty supports [Kitty's OSC 5522 clipboard protocol](https://sw.kovidgoyal.net/kitty/clipboard/).
On macOS it transfers plain text, HTML, image bytes, custom MIME formats, and MIME
aliases through the native pasteboard. Other platforms currently use the existing
text clipboard provider; binary or multiple-format writes return `ENOSYS` there.
The primary selection returns `ENOSYS` when the platform has no selection clipboard.

Supported operations include format discovery, reads, staged multi-format writes,
chunked Base64 data, `walias`, request IDs, and the protocol's error replies. Writes
reach the clipboard only after the final `wdata` packet; invalid or cancelled
packets discard the transaction. Both continuous Base64 fragments and individually
padded packets from existing Kitty clients are accepted. Invalid characters and
incomplete padding are rejected.

## Permissions and paste events

Clipboard requests honor `terminal.osc52` and require the target pane to be focused:

- `OnlyCopy` (default) permits writes and format discovery. Unsolicited data reads
  receive `EPERM`.
- `OnlyPaste` permits reads and format discovery.
- `CopyPaste` permits reads and writes.
- `Disabled` denies protocol access and disables paste-event mode.

Applications can query support with `CSI ? 5522 $ p`, enable paste notifications
with `CSI ? 5522 h`, and disable them with `CSI ? 5522 l`. When enabled, the Paste
shortcut and menu send available MIME types instead of inserting clipboard text.
Search fields and literal paste actions retain their normal text behavior.

Each notification includes a random password authorizing one read from that
clipboard in that pane for up to 60 seconds. The client returns it with a human
name, normally `Paste event`, as specified by Kitty. Terminal resets, disabling the
mode, and disabling clipboard access revoke these grants. A password on an
unsolicited request never grants access on its own; permission is controlled by
the configuration above rather than a dialog.

On macOS, exact MIME names are retained alongside native UTI representations so
native applications can use standard formats and terminal clients can recover
aliases accurately.

## Limits and validation

Clipboard writes support 64 MiB across their representations, at most 64 MIME
types, 256-byte MIME names, and 64 KiB per OSC packet. Oversized data returns
`EFBIG`; oversized or invalid packets return `EINVAL` and abort the write. Reply
data and format lists are sent in chunks of at most 4096 decoded bytes.

```sh
cargo test --locked -p alacritty_terminal clipboard
cargo test --locked -p alacritty_terminal --test kitty_clipboard
cargo test --locked -p alacritty clipboard
cargo test --locked -p vte
```

macOS native tests use named private pasteboards and leave the user's clipboard
untouched. The optional real-client test also uses a private pasteboard:

```sh
ALACRITTY_TEST_KITTEN=/absolute/path/to/kitten \
  cargo test --locked -p alacritty official_kitten_clipboard_round_trip -- --ignored
```

The official Kitty 0.49.2 client was checked for binary/text writes, aliases,
format discovery, and binary reads through a controlling PTY.

The protocol state is adapted from Termy commit
`9398de6031f13d0292347e8bdc3f31ecdc9a416c`. Its MIT notice is preserved in
`crates/terminal/src/clipboard/LICENSE-MIT` and packaged with the macOS app.
VTE dispatch keeps clipboard operations in order with text and synchronized output.
