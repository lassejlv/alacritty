#!/usr/bin/env python3
"""Exercise bundled zsh hooks in a real PTY without changing user dotfiles."""
import errno
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "assets/shell-integration/zsh"


def read_prompt(fd):
    data = b""
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            try:
                chunk = os.read(fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            data += chunk
            if b"\x1b]133;B\x07" in data:
                return data
    raise AssertionError(f"zsh prompt marker missing: {data!r}")


def main():
    zsh = shutil.which("zsh")
    if not zsh:
        raise SystemExit("zsh is required")
    with tempfile.TemporaryDirectory(prefix="alacritty-shell-test-") as temporary:
        root = Path(temporary)
        user = root / "user"
        bootstrap = root / "bootstrap"
        cwd = root / "hello æ;world"
        for directory in (user, bootstrap, cwd):
            directory.mkdir()
        (user / ".zshenv").write_text("export ENV_SEEN=yes\n")
        (user / ".zprofile").write_text("export PROFILE_SEEN=yes\n")
        (user / ".zshrc").write_text("export RC_SEEN=yes\nPS1='test> '\nHISTFILE=/dev/null\n")
        (user / ".zlogin").write_text("export LOGIN_SEEN=yes\n")
        for script in (".zshenv", "alacritty.zsh"):
            shutil.copyfile(SCRIPTS / script, bootstrap / script)
        env = dict(os.environ, TERM="xterm-256color", ZDOTDIR=str(bootstrap),
                   ALACRITTY_ZDOTDIR_SET="1", ALACRITTY_ZDOTDIR=str(user),
                   ALACRITTY_SHELL_INTEGRATION=str(bootstrap / "alacritty.zsh"))
        child, master = pty.fork()
        if child == 0:
            os.chdir(cwd)
            os.execvpe(zsh, [zsh, "-i", "-l"], env)
        try:
            first = read_prompt(master)
            assert b"hello%20%C3%A6%3Bworld" in first, first
            command = b"printf '__STARTUP__%s:%s:%s:%s:%s\\n' $ENV_SEEN $PROFILE_SEEN $RC_SEEN $LOGIN_SEEN $ZDOTDIR; false\n"
            os.write(master, command)
            output = read_prompt(master)
            assert b"__STARTUP__yes:yes:yes:yes:" + str(user).encode() in output, output
            assert b"\x1b]133;C\x07" in output, output
            assert b"\x1b]133;D;1\x07" in output, output
            os.write(master, b"source $ALACRITTY_SHELL_INTEGRATION; true\n")
            output = read_prompt(master)
            assert len(re.findall(b"\x1b\\]133;C\x07", output)) == 1, output
            assert len(re.findall(b"\x1b\\]133;A\x07", output)) == 1, output
            assert b"\x1b]133;D;0\x07" in output, output
            os.write(master, b"exit\n")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if os.waitpid(child, os.WNOHANG)[0]:
                    child = None
                    break
                if select.select([master], [], [], 0.1)[0]:
                    try:
                        os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
            assert child is None, "zsh did not exit"
        finally:
            if child is not None:
                os.kill(child, signal.SIGKILL)
                os.waitpid(child, 0)
            os.close(master)
    print("Zsh PTY integration passed: startup files, encoded directory, command status, repeat sourcing.")


if __name__ == "__main__":
    main()
