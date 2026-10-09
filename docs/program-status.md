# Program status

Alacritty supports [OSC 7501 revision 0.3](https://www.superlogical.com/rex/docs/build/program-status).
Each pane keeps its own records. With dynamic titles enabled, window and native
tab titles show the selected record's state, percentage, label, and message.
Blocked and error records take priority, followed by done, working, and idle.
The most recently updated record breaks ties. Titles are limited to 256 characters;
invisible direction controls are removed from displayed status text.

Try this inside Alacritty:

```sh
printf '\033]7501;state=working:app=build:progress=40\033\\'
sleep 2
printf '\033]7501;state=done:app=build:msg=QnVpbGQgY29tcGxldGU=\033\\'
sleep 2
printf '\033]7501;state=clear\033\\'
```

The title changes to `[working 40%] build`, then `[done] build: Build complete`,
then returns to the current shell title. Fixed titles configured with
`window.dynamic_title = false` or `--title` remain fixed.

Reports replace all fields on the addressed record. Omit `id` for the root;
use paths such as `build/tests` for child records. Clearing a parent clears its
children. Missing app names inherit from the nearest ancestor. Both BEL and
ESC-backslash terminators work. `OSC 7501 ; ? ST` receives the same query back,
and the terminfo entries advertise the `Pst` capability.

A shell prompt marker, `OSC 133 ; A ST`, or PTY process exit removes working and
blocked records. Done, error, and idle records remain until replaced or cleared.
A full terminal reset clears everything; screen switches and soft resets do not.
Alacritty stores up to 256 records and evicts the least recently updated record
when full. Oversized reports, invalid encoded text, and decoded control characters
leave existing records untouched.

Run the protocol and title-event tests with:

```sh
cargo test --locked -p alacritty_terminal --test program_status
```
