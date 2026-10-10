"""Drive terminals from Python, the way an agent would.

Exits with an assertion error when a check fails, so it doubles as a binding test.
"""

import sys

import alacritty_ffi
from alacritty_ffi import Terminal

print("alacritty_ffi", alacritty_ffi.version())

# Headless: feed program output and read the screen.
with Terminal.headless(columns=40, rows=8) as term:
    term.feed("\x1b]2;agent\x07\x1b]7;file://box/tmp\x07")
    term.feed("\x1b]133;A\x07$ \x1b]133;B\x07make\r\n\x1b]133;C\x07built ok\r\n\x1b]133;D;0\x07")
    term.feed("see https://example.com\r\n")
    events = term.events()
    assert any(event.kind == "working_directory_changed" for event in events), events
    assert term.title == "agent" and term.current_title() == "agent"
    assert term.working_directory() == ("box", "/tmp")
    state = term.shell_state()
    assert state["last_command"] == "make" and state["last_exit_status"] == 0, state
    assert term.last_command_output().strip() == "built ok"

    link = term.link_at(6, 2)
    assert link.uri == "https://example.com" and not link.hyperlink, link
    assert [link.uri for link in term.visible_links()] == ["https://example.com"]

    # Input becomes "output" events for the program on the other side.
    assert term.key("up")
    term.type("ls")
    output = b"".join(event.data for event in term.events() if event.kind == "output")
    assert output == b"\x1b[Als", output

    writer = term.writer()
    assert writer.write("from another thread")
    assert [event.data for event in term.events()] == [b"from another thread"]

    snapshot = term.snapshot()
    assert snapshot.row_text(1).startswith("built ok")
    term.feed("\x1b[8;1Hlast")
    term.update_snapshot(snapshot)
    assert not snapshot.info.full_damage and 7 in snapshot.damaged_rows()

    term.select_click(1, 1, clicks=2)
    assert term.selection_text() == "built"
    assert term.search("exa.ple").start_column == 12
    term.vi_toggle()
    term.vi_motion("first")
    assert term.mode & alacritty_ffi.MODE_VI

assert not writer.write("closed")

# PTY: run a program and collect its screen.
if sys.platform != "win32":
    with Terminal.spawn(["/bin/sh", "-c", "printf 'hello from the pty'; exit 3"]) as term:
        assert term.wait_for(lambda term: not term.running)
        assert "hello from the pty" in term.screen_text()
        assert term.exit_code == 3, term.exit_code

print("ok")
