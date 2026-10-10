# Desktop notifications

Alacritty supports [Kitty OSC 99 desktop notifications](https://sw.kovidgoyal.net/kitty/desktop-notifications/), using UserNotifications on macOS, toast notifications
on Windows, and the freedesktop notification service on Linux/BSD.

```sh
printf '\033]99;i=build:d=0:a=focus,report;Build complete\033\\'
printf '\033]99;i=build:p=body;All tests passed\033\\'
```

iTerm2-style `OSC 9` notifications are also supported. Their text becomes the
notification title, using the same delivery, focus and configuration as OSC 99.
OSC 9 bodies starting with a ConEmu sub-ID from 1 to 12, such as `9;4;…`
progress, are not notifications.

```sh
printf '\033]9;Build complete\033\\'
```

Clicking focuses the originating window, tab and pane. `a=report` also sends an
activation reply to that pane's PTY. `a=-focus` disables focus changes. Clicking
old, replaced notifications cannot activate a newer notification. Exiting the
pane or resetting the terminal closes its notifications.

## Supported controls

- Title/body chunks, UTF-8 and padded or unpadded Base64, including chunks split
  before or after encoding. Semicolons in payloads remain literal.
- `i` identifiers and `d=0`/`d=1` assembly. Reusing an identifier replaces its
  notification. Anonymous notifications remain independent.
- `p=buttons` with U+2028-separated labels; requested action replies use one-based
  button numbers. Windows displays up to five buttons, other platforms up to eight.
- `p=close`, `c=1` close reports, `p=alive` queries, and `p=?` capability queries.
- `o=always`, `o=unfocused`, `o=invisible`, urgency `u=0/1/2`, system/silent sound,
  and `w` expiry in milliseconds. `w=-1` uses OS policy; `w=0` requests no
  expiry where supported.
- Named theme icons on Linux/BSD and standard named symbols or application icons
  on macOS. Binary icon uploads and icon caching are not advertised or implemented.

The capability response lists the supported payloads and controls. Unknown keys
and payload extensions are ignored. Identifiers are validated before appearing
in replies. Notification text is plain text, never executable shell content.

## Configuration and limits

Set `terminal.desktop_notifications = false` to suppress requests and close
existing notifications. OS permissions and Focus/Do Not Disturb settings still
apply. macOS delivery requires the signed `.app` bundle; a plain `cargo run`
binary has no bundle identity. Permission is requested on the first notification.
Linux/BSD requires a running session notification service. Windows registers the
Alacritty notification identity under the current user's registry.

Each pane permits 32 partial and 32 active notifications. Partial messages expire
after 60 seconds. Each text field is limited to 32 KiB, each plain chunk to 2 KiB,
and each encoded chunk to 4 KiB. IDs are limited to 128 bytes. Delivery workers
are bounded, and callbacks are discarded after replacement, reset or pane exit.
macOS retains at most 64 distinct button-label sets per process. Native systems
may truncate text or limit notification presentation further.

## Validation

```sh
cargo test --locked -p alacritty_terminal --test notifications
```

For a manual check, send the example from two panes, click each notification,
and verify focus returns to its sender. Query `p=alive`, replace an ID, close it,
and test while the originating pane is maximized, hidden or closed.
