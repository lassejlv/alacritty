# Progress indicators

Alacritty implements [ConEmu OSC 9;4 progress](https://ghostty.org/docs/vt/osc/conemu).
A thin bar appears at the top of the originating pane. Normal progress uses the
theme's blue, errors use red, and paused work uses yellow. Indeterminate progress
animates. Inactive-pane dimming applies to the indicator too.

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
marker, PTY exit or full terminal reset clears progress. Applications should send
the hide sequence on completion; there is no automatic inactivity timeout.

BEL and ST are accepted. Other ConEmu commands are not executed.

Run `cargo test --locked -p alacritty_terminal --test shell_protocols` for parser
and state-transition coverage.
