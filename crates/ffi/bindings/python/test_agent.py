"""Checks the agent helpers and the recipes in docs/embedding/agents.md against real programs."""

import os
from pathlib import Path
import pty
import select
import shutil
import sys
import tempfile
import time

from agent import Shell, highlighted_rows, run, settle
from alacritty_ffi import MODE_ALT_SCREEN, Terminal

if sys.platform == "win32":
    print("skipped on Windows")
    sys.exit(0)

code, text = run(["/bin/sh", "-c", "seq 1 120; echo done; exit 4"], rows=10)
assert code == 4 and text.startswith("1\n2\n") and text.endswith("120\ndone"), (code, text[-40:])

if shutil.which("zsh"):
    with Shell() as shell:
        assert shell.run("cd /tmp && echo hi") == (0, "hi")
        assert shell.cwd in ("/tmp", "/private/tmp"), shell.cwd
        status, output = shell.run("ls /nonexistent-dir")
        assert status != 0 and "nonexistent-dir" in output, (status, output)
        assert shell.run_script("for i in 1 2; do\n  echo line $i\ndone") == (0, "line 1\nline 2")
        try:
            shell.run("sleep 30", timeout=0.5)
            raise AssertionError("expected a timeout")
        except TimeoutError:
            assert shell.term.shell_state()["phase"] == "output"
        assert shell.interrupt()
        assert shell.term.shell_state()["last_exit_status"] == 130
        assert shell.run("echo still usable") == (0, "still usable")
else:
    print("zsh not found, skipping Shell")

if shutil.which("less"):
    path = Path(tempfile.mkdtemp(), "notes.txt")
    path.write_text("".join(f"entry {i}\n" for i in range(200)))
    with Terminal.spawn(["less", str(path)], columns=80, rows=24, env={"LESS": ""}) as term:
        assert settle(term)
        assert term.mode & MODE_ALT_SCREEN
        assert term.screen_text().startswith("entry 0\n")
        term.type("/entry 150\r")
        settle(term)
        assert term.screen_text().startswith("entry 150"), term.screen_text()[:40]
        term.type("q")
        assert term.wait_for(lambda t: not t.running, 5)

with Terminal.headless(columns=20, rows=4) as term:
    term.feed("  apple\r\n\x1b[7m> banana\x1b[0m\r\n  cherry")
    assert highlighted_rows(term) == [(1, "> banana")], highlighted_rows(term)

# Headless proxy: the program runs elsewhere and bytes travel over any transport.
pid, fd = pty.fork()
if pid == 0:
    os.execvp("/bin/sh", ["/bin/sh", "-c", "printf 'name? '; read name; printf 'hi %s\\n' \"$name\""])
with Terminal.headless(columns=80, rows=24) as term:
    def pump(seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([fd], [], [], 0.05)[0]:
                try:
                    term.feed(os.read(fd, 65536))
                except OSError:
                    return
            for event in term.events():
                if event.kind == "output":
                    os.write(fd, event.data)

    pump(0.5)
    assert "name?" in term.screen_text()
    term.type("ada\r")
    pump(0.5)
    assert "hi ada" in term.screen_text(), term.screen_text()
os.waitpid(pid, 0)

print("ok")
