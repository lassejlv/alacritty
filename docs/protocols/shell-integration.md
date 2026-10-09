# Shell integration

Alacritty supports [OSC 133 semantic shell markers](https://iterm2.com/documentation-escape-codes.html#Shell_Integration_FinalTerm) and [OSC 7 working-directory reports](https://ghostty.org/docs/vt/osc/7).

## Using it

Interactive zsh sessions launched by Alacritty get integration automatically.
The app embeds the scripts, writes a private temporary startup directory for each
session, and restores the original `ZDOTDIR` before loading user configuration.
User dotfiles are not edited. Login startup files, prompt themes and existing
hooks remain in place. Non-shell commands, `zsh -c`, `zsh -f`, and custom shell
arguments are not intercepted.

Set `terminal.shell_integration = false` to disable automatic setup. A nested
interactive zsh can opt in with `source "$ALACRITTY_SHELL_INTEGRATION"` while its
original pane is alive. Sourcing it repeatedly does not duplicate hooks.
Other shells can emit the sequences below themselves; automatic injection is
currently provided for zsh.

Cmd+Shift+Up/Down on macOS, or Ctrl+Shift+Up/Down elsewhere, moves between commands.
The shortcuts leave alternate-screen apps and terminal search alone. The macOS
View menu also provides Previous Command and Next Command. Edit and the terminal
context menu provide Copy Last Command Output and Select Last Command Output.
Custom bindings can use `PreviousPrompt`, `NextPrompt`, `CopyLastCommandOutput`
and `SelectLastCommandOutput` on any platform.

## Command markers

| Sequence | Meaning |
| --- | --- |
| `OSC 133 ; A ST` | Start of a primary prompt |
| `OSC 133 ; B ST` | End of prompt, beginning of editable input |
| `OSC 133 ; C ST` | Start of command output |
| `OSC 133 ; D ; status ST` | End of output and exit status, 0–255 |

`D` without a status records an unknown result. `D` after `B` aborts the input.
`A;k=s` marks a secondary prompt without starting another command. Unknown
extensions are ignored. Markers are ignored on the alternate screen.

Boundaries move with grid cells through scrolling and reflow. Erasing or evicting
those cells invalidates output extraction rather than returning unrelated text.
Copy/select excludes the shell prompt and command itself. The terminal API exposes
the last command text, optional exit status, output and retained output range.
Captured command text is limited to 8 KiB. Markers use the configured scrollback
limit; no separate unbounded command history is stored.

A new primary prompt also clears transient OSC 7501 status and OSC 9;4 progress.

## Working directory

```sh
printf '\033]7;file://localhost/tmp/my%%20project\033\\'
```

The UTF-8 file URI is limited to 8 KiB. Percent escapes are decoded; malformed
escapes, controls, credentials and non-file schemes are rejected. The host is
retained. Only an empty host, `localhost`, or the current machine's hostname can
supply a local directory for a new tab, window or split, and that directory must
still exist. Remote paths are not treated as local folders. The existing process
inspection remains the fallback. This does not establish a new SSH connection.

BEL and ST terminators are accepted. Cancelled, oversized and unfinished packets
do not update state. A full terminal reset clears command and directory state.

## Validation

```sh
cargo test --locked -p alacritty_terminal --test shell_protocols
python3 scripts/smoke/shell-integration.py
```
