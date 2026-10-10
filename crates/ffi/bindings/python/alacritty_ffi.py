"""Python bindings for the Alacritty terminal engine C API, using ctypes.

The shared library is found through the ALACRITTY_FFI_LIBRARY environment variable, the system
library path, or a `target/release` or `target/debug` build of this repository.

    from alacritty_ffi import Terminal

    term = Terminal.spawn(["/bin/sh", "-c", "echo hi"])
    while term.running:
        term.wait()
    print(term.screen_text())
"""

import ctypes
import ctypes.util
import os
from pathlib import Path
import sys
import threading

__all__ = ["Terminal", "Event", "Link", "Snapshot", "Writer", "AlacError", "library"]

ABI_VERSION = 1

OSC52_DISABLED, OSC52_ONLY_COPY, OSC52_ONLY_PASTE, OSC52_COPY_PASTE = range(4)
KEY_PRESS, KEY_REPEAT, KEY_RELEASE = range(3)
MOD_SHIFT, MOD_ALT, MOD_CONTROL, MOD_SUPER = 1, 2, 4, 8
KEYS = {
    name: index + 1
    for index, name in enumerate(
        "enter tab backspace escape space insert delete home end page_up page_down up down left "
        "right".split()
    )
}
KEY_F1 = 101
SCROLL_DELTA, SCROLL_PAGE_UP, SCROLL_PAGE_DOWN, SCROLL_TOP, SCROLL_BOTTOM = range(5)
SELECTION_SIMPLE, SELECTION_BLOCK, SELECTION_SEMANTIC, SELECTION_LINES = range(4)
CLIPBOARD, CLIPBOARD_PRIMARY = range(2)
MODE_SHOW_CURSOR = 1 << 0
MODE_APP_CURSOR = 1 << 1
MODE_BRACKETED_PASTE = 1 << 4
MODE_MOUSE_REPORT_CLICK = 1 << 3
MODE_ALT_SCREEN = 1 << 12
MODE_VI = 1 << 16
CELL_INVERSE = 1 << 0
CELL_BOLD = 1 << 1
CELL_ITALIC = 1 << 2
CELL_UNDERLINE = 1 << 3
CELL_WIDE_CHAR = 1 << 5
CELL_WIDE_CHAR_SPACER = 1 << 6
CELL_DIM = 1 << 7
CELL_HIDDEN = 1 << 8
CELL_STRIKEOUT = 1 << 9
CELL_SELECTED = 1 << 16
CELL_DEFAULT_BACKGROUND = 1 << 17
SHELL_PHASES = ("unknown", "prompt", "input", "output")
VI_MOTIONS = (
    "up down left right first last first_occupied high middle low semantic_left semantic_right "
    "semantic_left_end semantic_right_end word_left word_right word_left_end word_right_end "
    "bracket paragraph_up paragraph_down"
).split()

EVENT_NAMES = {
    1: "wakeup",
    2: "title",
    3: "bell",
    4: "cursor_blinking_change",
    5: "mouse_cursor_dirty",
    6: "program_status_changed",
    7: "progress_changed",
    8: "notification_show",
    9: "notification_close",
    10: "notification_query_alive",
    11: "output",
    12: "child_exit",
    13: "exit",
    14: "working_directory_changed",
}


class AlacError(RuntimeError):
    pass


class Options(ctypes.Structure):
    _fields_ = [
        ("columns", ctypes.c_uint32),
        ("rows", ctypes.c_uint32),
        ("cell_width", ctypes.c_float),
        ("cell_height", ctypes.c_float),
        ("scrollback", ctypes.c_uint32),
        ("osc52", ctypes.c_uint32),
        ("cursor_shape", ctypes.c_uint32),
        ("cursor_blinking", ctypes.c_bool),
        ("kitty_keyboard", ctypes.c_bool),
        ("bold_is_bright", ctypes.c_bool),
        ("scroll_on_input", ctypes.c_bool),
        ("desktop_notifications", ctypes.c_bool),
        ("semantic_escape_chars", ctypes.c_char_p),
    ]


class PtyOptions(ctypes.Structure):
    _fields_ = [
        ("program", ctypes.c_char_p),
        ("args", ctypes.POINTER(ctypes.c_char_p)),
        ("arg_count", ctypes.c_size_t),
        ("working_directory", ctypes.c_char_p),
        ("env", ctypes.POINTER(ctypes.c_char_p)),
        ("env_count", ctypes.c_size_t),
        ("drain_on_exit", ctypes.c_bool),
    ]


class KeyEvent(ctypes.Structure):
    _fields_ = [
        ("action", ctypes.c_uint32),
        ("key", ctypes.c_uint32),
        ("codepoint", ctypes.c_uint32),
        ("unshifted_codepoint", ctypes.c_uint32),
        ("text", ctypes.c_char_p),
        ("location", ctypes.c_uint32),
        ("mods", ctypes.c_uint32),
        ("alt_composes", ctypes.c_bool),
    ]


class Progress(ctypes.Structure):
    _fields_ = [("state", ctypes.c_uint32), ("percent", ctypes.c_uint32)]


class Notification(ctypes.Structure):
    _fields_ = [
        ("serial", ctypes.c_uint64),
        ("id", ctypes.c_char_p),
        ("title", ctypes.c_char_p),
        ("body", ctypes.c_char_p),
        ("sound", ctypes.c_char_p),
        ("application", ctypes.c_char_p),
        ("buttons", ctypes.POINTER(ctypes.c_char_p)),
        ("button_count", ctypes.c_size_t),
        ("icons", ctypes.POINTER(ctypes.c_char_p)),
        ("icon_count", ctypes.c_size_t),
        ("urgency", ctypes.c_uint32),
        ("occasion", ctypes.c_uint32),
        ("expires_in_ms", ctypes.c_int64),
        ("focus", ctypes.c_bool),
        ("report", ctypes.c_bool),
        ("report_close", ctypes.c_bool),
        ("persistent", ctypes.c_bool),
    ]


class RawEvent(ctypes.Structure):
    _fields_ = [
        ("kind", ctypes.c_uint32),
        ("exit_code", ctypes.c_int32),
        ("serial", ctypes.c_uint64),
        ("data", ctypes.POINTER(ctypes.c_uint8)),
        ("len", ctypes.c_size_t),
        ("progress", Progress),
        ("notification", ctypes.POINTER(Notification)),
    ]


class Match(ctypes.Structure):
    _fields_ = [
        ("start_line", ctypes.c_int32),
        ("start_column", ctypes.c_uint32),
        ("end_line", ctypes.c_int32),
        ("end_column", ctypes.c_uint32),
    ]

    def __repr__(self):
        return (f"Match(({self.start_line}, {self.start_column}), "
                f"({self.end_line}, {self.end_column}))")


class ShellState(ctypes.Structure):
    _fields_ = [
        ("phase", ctypes.c_uint32),
        ("has_last_command", ctypes.c_bool),
        ("last_command_id", ctypes.c_uint64),
        ("last_exit_status", ctypes.c_int32),
    ]


class RawLink(ctypes.Structure):
    _fields_ = [("uri", ctypes.c_char_p), ("range", Match), ("hyperlink", ctypes.c_bool)]


class Cell(ctypes.Structure):
    _fields_ = [
        ("codepoint", ctypes.c_uint32),
        ("fg", ctypes.c_uint32),
        ("bg", ctypes.c_uint32),
        ("underline_color", ctypes.c_uint32),
        ("flags", ctypes.c_uint32),
        ("zerowidth_index", ctypes.c_uint32),
        ("zerowidth_count", ctypes.c_uint16),
        ("reserved", ctypes.c_uint16),
        ("hyperlink", ctypes.c_uint32),
    ]


class Cursor(ctypes.Structure):
    _fields_ = [
        ("column", ctypes.c_uint32),
        ("row", ctypes.c_uint32),
        ("shape", ctypes.c_uint32),
        ("color", ctypes.c_uint32),
        ("text_color", ctypes.c_uint32),
        ("blinking", ctypes.c_bool),
        ("wide", ctypes.c_bool),
    ]


class LineDamage(ctypes.Structure):
    _fields_ = [("row", ctypes.c_uint32), ("left", ctypes.c_uint32), ("right", ctypes.c_uint32)]


class SnapshotInfo(ctypes.Structure):
    _fields_ = [
        ("columns", ctypes.c_uint32),
        ("rows", ctypes.c_uint32),
        ("display_offset", ctypes.c_uint32),
        ("history_size", ctypes.c_uint32),
        ("mode", ctypes.c_uint32),
        ("foreground", ctypes.c_uint32),
        ("background", ctypes.c_uint32),
        ("full_damage", ctypes.c_bool),
        ("cursor", Cursor),
        ("cells", ctypes.POINTER(Cell)),
        ("codepoints", ctypes.POINTER(ctypes.c_uint32)),
        ("codepoint_count", ctypes.c_size_t),
        ("damage", ctypes.POINTER(LineDamage)),
        ("damage_count", ctypes.c_size_t),
        ("images", ctypes.c_void_p),
        ("image_count", ctypes.c_size_t),
    ]


WAKEUP = ctypes.CFUNCTYPE(None, ctypes.c_void_p)

_SIGNATURES = {
    "alac_abi_version": (ctypes.c_uint32, []),
    "alac_version": (ctypes.c_char_p, []),
    "alac_last_error": (ctypes.c_char_p, []),
    "alac_string_free": (None, [ctypes.c_void_p]),
    "alac_options_default": (None, [ctypes.POINTER(Options)]),
    "alac_terminal_new_headless": (ctypes.c_void_p, [ctypes.POINTER(Options), WAKEUP, ctypes.c_void_p]),
    "alac_terminal_spawn": (ctypes.c_void_p, [ctypes.POINTER(Options), ctypes.POINTER(PtyOptions),
                                              WAKEUP, ctypes.c_void_p]),
    "alac_terminal_free": (None, [ctypes.c_void_p]),
    "alac_terminal_child_pid": (ctypes.c_int64, [ctypes.c_void_p]),
    "alac_terminal_feed": (ctypes.c_bool, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]),
    "alac_terminal_write": (None, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]),
    "alac_terminal_text": (None, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]),
    "alac_terminal_paste": (None, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]),
    "alac_terminal_key": (ctypes.c_bool, [ctypes.c_void_p, ctypes.POINTER(KeyEvent)]),
    "alac_terminal_resize": (None, [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
                                    ctypes.c_float, ctypes.c_float]),
    "alac_terminal_set_focused": (None, [ctypes.c_void_p, ctypes.c_bool]),
    "alac_terminal_mode": (ctypes.c_uint32, [ctypes.c_void_p]),
    "alac_terminal_scroll": (None, [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int32]),
    "alac_terminal_poll_event": (ctypes.c_bool, [ctypes.c_void_p, ctypes.POINTER(RawEvent)]),
    "alac_terminal_next_timeout_ms": (ctypes.c_int64, [ctypes.c_void_p]),
    "alac_terminal_tick": (None, [ctypes.c_void_p]),
    "alac_terminal_snapshot": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_snapshot_new": (ctypes.c_void_p, []),
    "alac_terminal_snapshot_update": (ctypes.c_bool, [ctypes.c_void_p, ctypes.c_void_p]),
    "alac_snapshot_free": (None, [ctypes.c_void_p]),
    "alac_snapshot_info": (None, [ctypes.c_void_p, ctypes.POINTER(SnapshotInfo)]),
    "alac_terminal_screen_text": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_terminal_text_range": (ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_int32, ctypes.c_int32]),
    "alac_terminal_history_size": (ctypes.c_uint32, [ctypes.c_void_p]),
    "alac_terminal_select_start": (None, [ctypes.c_void_p] + [ctypes.c_uint32] * 4),
    "alac_terminal_select_update": (None, [ctypes.c_void_p] + [ctypes.c_uint32] * 3),
    "alac_terminal_select_click": (None, [ctypes.c_void_p] + [ctypes.c_uint32] * 4 + [ctypes.c_bool]),
    "alac_terminal_select_all": (None, [ctypes.c_void_p]),
    "alac_terminal_select_clear": (None, [ctypes.c_void_p]),
    "alac_terminal_selection_text": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_terminal_search": (ctypes.c_int32, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int32,
                                              ctypes.POINTER(Match)]),
    "alac_terminal_search_clear": (None, [ctypes.c_void_p]),
    "alac_terminal_title": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_terminal_working_directory": (ctypes.c_void_p, [ctypes.c_void_p,
                                                          ctypes.POINTER(ctypes.c_void_p)]),
    "alac_terminal_shell_state": (None, [ctypes.c_void_p, ctypes.POINTER(ShellState)]),
    "alac_terminal_last_command": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_terminal_last_command_output": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_terminal_link_at": (ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
                                                ctypes.POINTER(Match), ctypes.POINTER(ctypes.c_bool)]),
    "alac_terminal_set_url_pattern": (ctypes.c_bool, [ctypes.c_void_p, ctypes.c_char_p]),
    "alac_terminal_visible_links": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_link_list_len": (ctypes.c_size_t, [ctypes.c_void_p]),
    "alac_link_list_get": (ctypes.POINTER(RawLink), [ctypes.c_void_p, ctypes.c_size_t]),
    "alac_link_list_free": (None, [ctypes.c_void_p]),
    "alac_terminal_vi_toggle": (None, [ctypes.c_void_p]),
    "alac_terminal_vi_motion": (None, [ctypes.c_void_p, ctypes.c_uint32]),
    "alac_terminal_vi_select": (None, [ctypes.c_void_p, ctypes.c_uint32]),
    "alac_terminal_writer": (ctypes.c_void_p, [ctypes.c_void_p]),
    "alac_writer_write": (ctypes.c_bool, [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]),
    "alac_writer_free": (None, [ctypes.c_void_p]),
}


def _candidates():
    if os.environ.get("ALACRITTY_FFI_LIBRARY"):
        yield os.environ["ALACRITTY_FFI_LIBRARY"]
        return
    found = ctypes.util.find_library("alacritty_ffi")
    if found:
        yield found
    name = {"darwin": "libalacritty_ffi.dylib", "win32": "alacritty_ffi.dll"}.get(
        sys.platform, "libalacritty_ffi.so")
    root = Path(__file__).resolve().parents[4]
    for profile in ("release", "debug"):
        yield str(root / "target" / profile / name)


def _load():
    errors = []
    for candidate in _candidates():
        try:
            lib = ctypes.CDLL(candidate)
        except OSError as error:
            errors.append(str(error))
            continue
        for name, (restype, argtypes) in _SIGNATURES.items():
            function = getattr(lib, name)
            function.restype = restype
            function.argtypes = argtypes
        if lib.alac_abi_version() != ABI_VERSION:
            raise AlacError(f"{candidate} has ABI {lib.alac_abi_version()}, expected {ABI_VERSION}")
        return lib
    raise AlacError("alacritty_ffi library not found; set ALACRITTY_FFI_LIBRARY\n" + "\n".join(errors))


_lib = None


def library():
    """The loaded C library."""
    global _lib
    if _lib is None:
        _lib = _load()
    return _lib


def _take_string(pointer):
    """Decode and free a `char *` owned by the caller."""
    if not pointer:
        return None
    try:
        return ctypes.string_at(pointer).decode("utf-8", "replace")
    finally:
        library().alac_string_free(pointer)


def _error(default="call failed"):
    message = library().alac_last_error()
    return AlacError(message.decode() if message else default)


class Event:
    """A host event. `data` holds output bytes or decoded text, depending on `kind`."""

    def __init__(self, raw):
        self.kind = EVENT_NAMES.get(raw.kind, str(raw.kind))
        self.data = ctypes.string_at(raw.data, raw.len) if raw.data else None
        if self.kind != "output" and self.data is not None:
            self.data = self.data.decode("utf-8", "replace")
        self.exit_code = raw.exit_code
        self.serial = raw.serial
        self.progress = (raw.progress.state, raw.progress.percent)
        self.notification = None
        if raw.notification:
            notification = raw.notification.contents
            self.notification = {
                "title": (notification.title or b"").decode(),
                "body": (notification.body or b"").decode(),
            }

    def __repr__(self):
        return f"Event({self.kind!r}, {self.data!r})"


class Link:
    def __init__(self, uri, match, hyperlink):
        self.uri, self.range, self.hyperlink = uri, match, hyperlink

    def __repr__(self):
        return f"Link({self.uri!r}, {self.range!r}, hyperlink={self.hyperlink})"


class Snapshot:
    """Visible content, refreshed in place with `Terminal.update_snapshot`."""

    def __init__(self):
        self._handle = library().alac_snapshot_new()
        self.info = SnapshotInfo()

    def _refresh(self):
        library().alac_snapshot_info(self._handle, ctypes.byref(self.info))

    def cell(self, column, row):
        return self.info.cells[row * self.info.columns + column]

    def row_text(self, row):
        return "".join(chr(self.cell(column, row).codepoint) for column in range(self.info.columns))

    def damaged_rows(self):
        if self.info.full_damage:
            return list(range(self.info.rows))
        return [self.info.damage[i].row for i in range(self.info.damage_count)]

    def close(self):
        if self._handle:
            library().alac_snapshot_free(self._handle)
            self._handle = None

    __del__ = close


class Writer:
    """Thread-safe input for the child process, valid after the terminal is closed."""

    def __init__(self, handle):
        self._handle = handle

    def write(self, data):
        data = data.encode() if isinstance(data, str) else bytes(data)
        return library().alac_writer_write(self._handle, data, len(data))

    def close(self):
        if self._handle:
            library().alac_writer_free(self._handle)
            self._handle = None

    __del__ = close


class Terminal:
    """One terminal. Use it from one thread at a time; use `writer()` for other threads."""

    def __init__(self, handle, wakeup, ready):
        self._handle = handle
        self._wakeup = wakeup
        self._ready = ready
        self.running = True
        self.exit_code = None
        self.title = None

    @staticmethod
    def _options(columns, rows, **options):
        lib = library()
        raw = Options()
        lib.alac_options_default(ctypes.byref(raw))
        raw.columns, raw.rows = columns, rows
        for name, value in options.items():
            setattr(raw, name, value)
        return raw

    @classmethod
    def _create(cls, create):
        ready = threading.Event()
        wakeup = WAKEUP(lambda _userdata: ready.set())
        handle = create(wakeup)
        if not handle:
            raise _error("failed to create terminal")
        return cls(handle, wakeup, ready)

    @classmethod
    def headless(cls, columns=80, rows=24, **options):
        """Terminal parsing bytes passed to `feed`; its input arrives as "output" events."""
        raw = cls._options(columns, rows, **options)
        return cls._create(
            lambda wakeup: library().alac_terminal_new_headless(ctypes.byref(raw), wakeup, None))

    @classmethod
    def spawn(cls, argv=None, columns=80, rows=24, cwd=None, env=None, drain_on_exit=True,
              **options):
        """Terminal running `argv`, or the login shell, on a new PTY.

        `env` entries are added to the inherited environment. With `drain_on_exit`, output
        written just before the child exits is read before the exit is reported."""
        raw = cls._options(columns, rows, **options)
        pty = PtyOptions(drain_on_exit=drain_on_exit)
        keep = []
        if argv:
            pty.program = argv[0].encode()
            args = (ctypes.c_char_p * len(argv[1:]))(*[arg.encode() for arg in argv[1:]])
            pty.args, pty.arg_count = args, len(argv[1:])
            keep.append(args)
        if cwd:
            pty.working_directory = os.fsencode(cwd)
        if env:
            entries = (ctypes.c_char_p * len(env))(*[f"{k}={v}".encode() for k, v in env.items()])
            pty.env, pty.env_count = entries, len(env)
            keep.append(entries)
        return cls._create(lambda wakeup: library().alac_terminal_spawn(
            ctypes.byref(raw), ctypes.byref(pty), wakeup, None))

    def close(self):
        if self._handle:
            library().alac_terminal_free(self._handle)
            self._handle = None

    __del__ = close

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    @property
    def pid(self):
        pid = library().alac_terminal_child_pid(self._handle)
        return None if pid < 0 else pid

    # Events.

    def events(self):
        """Drain pending events, answering protocol queries on the way."""
        lib = library()
        raw = RawEvent()
        events = []
        while lib.alac_terminal_poll_event(self._handle, ctypes.byref(raw)):
            event = Event(raw)
            if event.kind == "title":
                self.title = event.data
            elif event.kind in ("child_exit", "exit"):
                self.running = False
                if event.kind == "child_exit":
                    self.exit_code = None if event.exit_code < 0 else event.exit_code
            events.append(event)
        return events

    def wait(self, timeout=None):
        """Block until the terminal has events or timers, then return the events."""
        lib = library()
        deadline = lib.alac_terminal_next_timeout_ms(self._handle)
        if deadline >= 0:
            timeout = deadline / 1000 if timeout is None else min(timeout, deadline / 1000)
        self._ready.wait(timeout)
        self._ready.clear()
        lib.alac_terminal_tick(self._handle)
        return self.events()

    def wait_for(self, predicate, timeout=10.0):
        """Process events until `predicate(self)` is true. Returns whether it became true."""
        import time

        end = time.monotonic() + timeout
        while not predicate(self):
            remaining = end - time.monotonic()
            if remaining <= 0:
                return False
            self.wait(remaining)
        return True

    # Input.

    def feed(self, data):
        """Parse child output for a headless terminal."""
        data = data.encode() if isinstance(data, str) else bytes(data)
        if not library().alac_terminal_feed(self._handle, data, len(data)):
            raise AlacError("feed requires a headless terminal")

    def write(self, data):
        """Send raw bytes to the child."""
        data = data.encode() if isinstance(data, str) else bytes(data)
        library().alac_terminal_write(self._handle, data, len(data))

    def type(self, text):
        """Send text as typed input."""
        data = text.encode()
        library().alac_terminal_text(self._handle, data, len(data))

    def paste(self, text):
        data = text.encode()
        library().alac_terminal_paste(self._handle, data, len(data))

    def key(self, key, mods=0, text=None, action=KEY_PRESS):
        """Press a named key ("enter", "up", "f5") or a character, encoded for the active
        keyboard protocol. Returns whether the key was used."""
        event = KeyEvent(action=action, mods=mods)
        if len(key) == 1:
            event.codepoint = ord(key)
        elif key.lower().startswith("f") and key[1:].isdigit():
            event.key = KEY_F1 + int(key[1:]) - 1
        else:
            event.key = KEYS[key.lower()]
        if text is not None:
            event.text = text.encode()
        return library().alac_terminal_key(self._handle, ctypes.byref(event))

    def writer(self):
        return Writer(library().alac_terminal_writer(self._handle))

    def resize(self, columns, rows, cell_width=8.0, cell_height=16.0):
        library().alac_terminal_resize(self._handle, columns, rows, cell_width, cell_height)

    def set_focused(self, focused):
        library().alac_terminal_set_focused(self._handle, focused)

    def scroll(self, lines):
        library().alac_terminal_scroll(self._handle, SCROLL_DELTA, lines)

    @property
    def mode(self):
        return library().alac_terminal_mode(self._handle)

    # Content.

    def screen_text(self):
        return _take_string(library().alac_terminal_screen_text(self._handle))

    def text_range(self, start_line, end_line):
        """Text between grid lines; negative lines are history and 0 is the top of the screen."""
        return _take_string(library().alac_terminal_text_range(self._handle, start_line, end_line))

    @property
    def history_size(self):
        return library().alac_terminal_history_size(self._handle)

    def snapshot(self):
        snapshot = Snapshot()
        self.update_snapshot(snapshot)
        return snapshot

    def update_snapshot(self, snapshot):
        """Refresh `snapshot`, copying only damaged rows when possible."""
        library().alac_terminal_snapshot_update(self._handle, snapshot._handle)
        snapshot._refresh()
        return snapshot

    # Selection and search.

    def select(self, start, end, kind=SELECTION_SIMPLE):
        """Select from `(column, row)` to `(column, row)` in the viewport, inclusive."""
        lib = library()
        lib.alac_terminal_select_start(self._handle, kind, start[0], start[1], 0)
        lib.alac_terminal_select_update(self._handle, end[0], end[1], 1)

    def select_click(self, column, row, clicks=1, block=False):
        library().alac_terminal_select_click(self._handle, column, row, 0, clicks, block)

    def select_all(self):
        library().alac_terminal_select_all(self._handle)

    def clear_selection(self):
        library().alac_terminal_select_clear(self._handle)

    def selection_text(self):
        return _take_string(library().alac_terminal_selection_text(self._handle))

    def search(self, pattern, backward=False):
        """Select and return the next regex match, or None."""
        match = Match()
        found = library().alac_terminal_search(
            self._handle, pattern.encode(), -1 if backward else 1, ctypes.byref(match))
        if found < 0:
            raise _error("invalid pattern")
        return match if found else None

    def clear_search(self):
        library().alac_terminal_search_clear(self._handle)

    # Links.

    def link_at(self, column, row):
        match, hyperlink = Match(), ctypes.c_bool()
        uri = _take_string(library().alac_terminal_link_at(
            self._handle, column, row, ctypes.byref(match), ctypes.byref(hyperlink)))
        return Link(uri, match, hyperlink.value) if uri else None

    def visible_links(self):
        lib = library()
        handle = lib.alac_terminal_visible_links(self._handle)
        try:
            links = []
            for index in range(lib.alac_link_list_len(handle)):
                raw = lib.alac_link_list_get(handle, index).contents
                links.append(Link(raw.uri.decode(), Match.from_buffer_copy(raw.range), raw.hyperlink))
            return links
        finally:
            lib.alac_link_list_free(handle)

    def set_url_pattern(self, pattern):
        """Regex for URL detection; None restores the default and "" disables detection."""
        pattern = None if pattern is None else pattern.encode()
        if not library().alac_terminal_set_url_pattern(self._handle, pattern):
            raise _error("invalid pattern")

    # Vi mode.

    def vi_toggle(self):
        library().alac_terminal_vi_toggle(self._handle)

    def vi_motion(self, motion):
        library().alac_terminal_vi_motion(self._handle, VI_MOTIONS.index(motion))

    def vi_select(self, kind=SELECTION_SIMPLE):
        library().alac_terminal_vi_select(self._handle, kind)

    # Protocol state.

    def current_title(self):
        return _take_string(library().alac_terminal_title(self._handle))

    def working_directory(self):
        """OSC 7 `(host, path)`, or None."""
        host = ctypes.c_void_p()
        path = _take_string(library().alac_terminal_working_directory(self._handle,
                                                                      ctypes.byref(host)))
        host = _take_string(host.value)
        return (host, path) if path is not None else None

    def shell_state(self):
        """OSC 133 shell integration state."""
        state = ShellState()
        library().alac_terminal_shell_state(self._handle, ctypes.byref(state))
        return {
            "phase": SHELL_PHASES[state.phase] if state.phase < len(SHELL_PHASES) else "unknown",
            "last_command": _take_string(library().alac_terminal_last_command(self._handle)),
            "last_command_id": state.last_command_id if state.has_last_command else None,
            "last_exit_status": None if state.last_exit_status < 0 else state.last_exit_status,
        }

    def last_command_output(self):
        return _take_string(library().alac_terminal_last_command_output(self._handle))


def version():
    return library().alac_version().decode()
