# Kitty graphics protocol

Alacritty supports the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
in each terminal pane. Clients can probe support using the standard `a=q` query;
no terminal-name override is required.

Supported operations:

- Direct RGB, RGBA, and PNG uploads, padded/unpadded Base64 chunks, and zlib compression.
- Local regular files, temporary files, and POSIX/Windows shared memory,
  including byte ranges and offsets.
- Separate upload/display, image IDs and image numbers, multiple placements,
  replacement, cropping, pixel offsets, aspect-preserving scaling, cursor
  movement, quiet replies, and all deletion selectors.
- Positive/negative image layers, including images below non-default cell
  backgrounds, and alpha blending.
- Unicode placeholders, omitted-diacritic continuation, high image-ID bytes,
  and underline-color placement selection.
- Relative placements, signed offsets, parent lifetime/cycle validation, and
  chains up to eight levels.
- Animation frame uploads, patches, composition, frame selection/deletion,
  frame delays, loading/running/stopped states, and loop counts.
- Primary/alternate buffers, scrollback, scrolling margins, terminal resets,
  and clear-screen handling. Other text erase operations leave direct graphics
  alone; erasing placeholder text removes its visible image cells.
- Nested split panes: image IDs, uploads, placement state, and animation timers
  remain independent. Graphics are clipped to their pane's text area.

The primary device-attributes reply advertises ANSI color so clients such as
`kitten icat --detect-support` recognize the end of capability probing. Successful
animation-control and delete commands remain silent, matching Kitty.

The optional `N` usage hint is accepted without changing eviction policy.
As with Kitty itself, resource limits apply: 128 MiB of decoded image/frame
storage per pane, dimensions up to 32768 pixels, 4096 images/placements, and a
192 MiB limit on an individual APC command. Protocol-compliant 4096-byte chunks
are supported within the same storage budget. The shared GPU texture
cache is bounded separately; large images use tiles with repeated edge pixels.

File transfers follow symlinks but reject special files and sensitive device
paths. File-read failures return the same `EBADF` response. Temporary-file
deletion is limited to recognized temporary directories and names containing
`tty-graphics-protocol`. No shell command from a graphics payload is executed.

## Validation

```sh
cargo test --locked -p alacritty_terminal
cargo test --locked -p alacritty renderer::graphics
cargo test --locked -p vte
```

Run the self-contained visual and transport fixture inside Alacritty:

```sh
python3 scripts/smoke/kitty-graphics.py
```

The fixture displays chunked PNG, compressed RGBA/crops, both text layering
orders, Unicode placeholders, relative placements, and a red/blue animation.
It also probes direct, file, temporary-file, and shared-memory transfers.
`--report PATH` writes machine-readable probe results. Creating a split while
the fixture is configured as the test shell exercises duplicate image IDs and
independent animations across panes.

The default macOS GLSL3 renderer is the native validation target. Forced
GLES2Pure rendering on the tested macOS Metal driver already loses text in
v0.18.2, and forced GLES2 with dual-source blending fails its existing text
shader's extension check there. These pre-existing forced-renderer limitations
are separate from graphics protocol support; the normal renderer is unaffected.

The official Kitty 0.49.2 `kitten icat` client was also checked with automatic
capability detection, stream transfer, file-backed Unicode placeholders, shared
memory, and animated GIFs. Automatic detection selects stream mode; explicit
file and shared-memory transfers pass too. A single 13.3 MB encoded upload of
a 5000-pixel-wide image also exercises large APC commands and GPU tile boundaries.

## Source and parser integration

The renderer-neutral protocol, image, geometry, and animation code is adapted
from Termy commit `9398de6031f13d0292347e8bdc3f31ecdc9a416c`, under the MIT license
in `crates/terminal/src/graphics/LICENSE-MIT`. The Mac app includes that
notice as `Kitty-Graphics-LICENSE.txt`.

The pinned VTE 0.15.0 source is vendored with an APC callback. This keeps image
commands within the same parser and synchronized-output buffer as text,
including reply ordering, cancellation, and partial-read handling. The local
patch is described in `vendor/vte/ALACRITTY-PATCH.md`; upstream licenses and
parser tests are preserved.
