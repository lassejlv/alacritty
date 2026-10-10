# Progress indicators

Alacritty implements [ConEmu OSC 9;4 progress](https://ghostty.org/docs/vt/osc/conemu).
A thin bar appears at the top of the originating pane. Normal progress uses the
theme's blue, errors use red, and paused work uses yellow. Indeterminate progress
animates. Inactive-pane dimming applies to the indicator too.

Progress is also shown natively: on macOS as a bar over the Dock icon, combining
all windows, and on Windows on each window's taskbar button. On Linux/BSD,
app-wide progress is sent through the Unity LauncherEntry D-Bus API for the
_Alacritty.desktop_ entry, which docks and task managers such as KDE Plasma and
Dash to Dock display. Launchers have no error color or indeterminate animation,
so errors mark the launcher entry urgent and indeterminate progress is not shown
there. When several panes report progress, errors take precedence, then paused,
normal and indeterminate work; panes in the same state show the least complete
one.

```sh
printf '\033]9;4;1;40\033\\'  # 40 percent
printf '\033]9;4;2\033\\'     # Error, retaining 40 percent
printf '\033]9;4;4\033\\'     # Paused, retaining 40 percent
printf '\033]9;4;3\033\\'     # Indeterminate
printf '\033]9;4;0\033\\'     # Hide
```

States are 0 hidden, 1 normal, 2 error, 3 indeterminate and 4 paused.
Values above 100 are clamped. Normal progress without a value starts at zero;
error and paused states without a value retain the previous percentage.
Indeterminate progress retains the stored percentage for subsequent updates.
Invalid states and values leave the previous state unchanged.

Progress belongs to the pane and is separate from OSC 7501 records. Reporting or
clearing OSC 9;4 does not overwrite a program's OSC 7501 status. A shell prompt
marker, PTY exit or full terminal reset clears progress. Like Ghostty, progress is
also hidden after 15 seconds without an OSC 9;4 update, so applications should
resend their state as a keep-alive (at least once a second is recommended) and
send the hide sequence on completion.

BEL and ST are accepted. Other ConEmu commands (sub-IDs 1 to 12) are ignored. OSC 9
text without such a sub-ID is an iTerm2-style desktop notification; see
[notifications](notifications.md).

Run `cargo test --locked -p alacritty_terminal --test shell_protocols` for parser
and state-transition coverage.
