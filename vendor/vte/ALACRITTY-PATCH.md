VTE 0.15.0 from crates.io. Local changes add bounded APC dispatch to Perform and ansi::Handler, preserving the existing parser and synchronized-output ordering. See LICENSE-APACHE and LICENSE-MIT.

Kitty clipboard support adds OSC 5522 dispatch and private mode 5522. Clipboard
OSC packets are bounded to 64 KiB and wait for the full ST before dispatch;
oversized, cancelled, or malformed control packets cannot commit a write.
Other OSC commands retain the upstream behavior.

OSC 7501 adds program status dispatch with a bounded buffer and complete-ST
validation. Its body preserves semicolons so the receiver can skip malformed
pairs. OSC 7, 9, 99 and 133 preserve their payload separators and use bounded
buffers with complete-ST validation. OSC 7 reports working directories, OSC 9;4
reports progress, OSC 99 dispatches desktop notification requests, and OSC 133
dispatches prompt, input, output and command-completion boundaries. Cancelled or
oversized OSC 99 requests invalidate incomplete multipart notifications.
