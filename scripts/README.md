Scripts
=======

## Flamegraph

Run the release version of Alacritty while recording call stacks. After the
Alacritty process exits, a flamegraph will be generated and it's URI printed
as the only output to STDOUT.

```sh
scripts/diagnostics/create-flamegraph.sh
```

Running this script depends on an installation of `perf`.

## ANSI Color Tests

We include a few scripts for testing the color of text inside a terminal. The
first shows various foreground and background variants. The second enumerates
all the colors of a standard terminal. The third enumerates the 24-bit colors.

```sh
scripts/diagnostics/fg-bg.sh
scripts/diagnostics/colors.sh
scripts/diagnostics/24-bit-color.sh
```

Run these commands from the repository root. Protocol smoke checks live in
`scripts/smoke/`; packaging and signing scripts live in `scripts/release/`.
