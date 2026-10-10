"""Helpers for agents that drive programs through Alacritty terminals.

    from agent import Shell, run

    status, output = run(["make", "test"], timeout=600)

    with Shell(cwd="/srv/app") as shell:
        status, output = shell.run("git status --short")

See docs/embedding/agents.md for the reasoning behind each helper.
"""

import os
from pathlib import Path
import shutil
import tempfile
import time

from alacritty_ffi import CELL_DEFAULT_BACKGROUND, CELL_INVERSE, MOD_CONTROL, Terminal

__all__ = ["run", "Shell", "settle", "highlighted_rows", "ZSH_INTEGRATION"]

# The zsh hooks bundled with Alacritty, emitting OSC 7 and OSC 133.
ZSH_INTEGRATION = Path(
    os.environ.get(
        "ALACRITTY_ZSH_INTEGRATION",
        Path(__file__).resolve().parents[4] / "assets/shell-integration/zsh/alacritty.zsh",
    )
)


def run(argv, timeout=120, columns=200, rows=50, cwd=None, env=None):
    """Run a program on its own PTY and return its exit code and everything it printed.

    Output beyond the scrollback (10,000 lines by default) is dropped from the start.
    """
    with Terminal.spawn(argv, columns=columns, rows=rows, cwd=cwd, env=env) as term:
        if not term.wait_for(lambda t: not t.running, timeout):
            raise TimeoutError(f"{argv} still running after {timeout}s")
        text = term.text_range(-term.history_size, rows - 1).rstrip()
        return term.exit_code, text


class Shell:
    """An interactive zsh whose commands are tracked with OSC 133 shell integration.

    User dotfiles are not loaded, so prompts and aliases are predictable.
    """

    def __init__(self, columns=200, rows=50, cwd=None, env=None, zsh=None, timeout=10):
        zsh = zsh or shutil.which("zsh")
        if not zsh:
            raise FileNotFoundError("zsh is required for Shell")
        if not ZSH_INTEGRATION.is_file():
            raise FileNotFoundError(f"set ALACRITTY_ZSH_INTEGRATION; {ZSH_INTEGRATION} missing")
        self._home = tempfile.TemporaryDirectory(prefix="alacritty-agent-")
        Path(self._home.name, ".zshrc").write_text(
            f"PS1='$ '\nHISTFILE=/dev/null\nsource '{ZSH_INTEGRATION}'\n")
        env = dict(env or {}, ZDOTDIR=self._home.name)
        self.term = Terminal.spawn([zsh, "-i"], columns=columns, rows=rows, cwd=cwd, env=env)
        if not self.term.wait_for(lambda t: t.shell_state()["phase"] == "input", timeout):
            self.close()
            raise TimeoutError("shell prompt did not appear")

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def _submit(self, timeout, describe):
        before = self.term.shell_state()["last_command_id"]
        self.term.type("\r")

        def finished(term):
            state = term.shell_state()
            return state["last_command_id"] != before and state["phase"] == "input"

        if not self.term.wait_for(finished, timeout):
            raise TimeoutError(f"{describe} still running after {timeout}s")
        state = self.term.shell_state()
        return state["last_exit_status"], self.term.last_command_output() or ""

    def run(self, command, timeout=60):
        """Run one command line and return `(exit status, output)`.

        On timeout the command keeps running; call `interrupt()` or keep waiting with
        `term.wait_for`.
        """
        self.term.type(command)
        return self._submit(timeout, repr(command))

    def run_script(self, script, timeout=60):
        """Run multi-line shell code, pasted so newlines don't submit it early."""
        self.term.paste(script)
        return self._submit(timeout, "script")

    def interrupt(self, timeout=5):
        """Send Ctrl+C and wait for the prompt."""
        self.term.key("c", MOD_CONTROL)
        return self.term.wait_for(lambda t: t.shell_state()["phase"] == "input", timeout)

    @property
    def cwd(self):
        directory = self.term.working_directory()
        return directory[1] if directory else None

    def close(self):
        self.term.close()
        self._home.cleanup()


def settle(term, quiet=0.3, timeout=10):
    """Wait until the program stops drawing for `quiet` seconds. Returns False on timeout."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if not term.wait(min(quiet, end - time.monotonic())):
            return True
    return False


def highlighted_rows(term):
    """Visible rows drawn in reverse video or with a non-default background, like the selected
    item of a menu."""
    snapshot = term.snapshot()
    rows = []
    for row in range(snapshot.info.rows):
        cells = (snapshot.cell(column, row) for column in range(snapshot.info.columns))
        if any(cell.codepoint > 32
               and (cell.flags & CELL_INVERSE or not cell.flags & CELL_DEFAULT_BACKGROUND)
               for cell in cells):
            rows.append((row, snapshot.row_text(row).rstrip()))
    return rows
