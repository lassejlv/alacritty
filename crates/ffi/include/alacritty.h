/*
 * Alacritty terminal engine C API.
 *
 * Embed the Alacritty terminal emulator in a custom terminal: the library runs the child
 * process, parses its output, answers terminal protocol queries and encodes keyboard and mouse
 * input. Hosts draw snapshots and provide windows, fonts and native clipboard access.
 *
 * Threading: a terminal handle must only be used from one thread at a time. The wakeup
 * callback can run on any thread while the terminal is locked; it must only schedule a call to
 * alac_terminal_poll_event() on the host thread and never call into this library. Use an
 * AlacWriter to send input from other threads.
 *
 * Memory: `char *` results are owned by the caller and released with alac_string_free().
 * Pointers inside events stay valid until the next alac_terminal_poll_event() call; pointers
 * inside snapshots stay valid until alac_snapshot_free().
 *
 * Colors are packed as 0xRRGGBB.
 */

#ifndef ALACRITTY_H
#define ALACRITTY_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ALAC_ABI_VERSION 1

typedef struct AlacTerminal AlacTerminal;
typedef struct AlacSnapshot AlacSnapshot;
typedef struct AlacClipboardSink AlacClipboardSink;
typedef struct AlacProgramStatusList AlacProgramStatusList;
typedef struct AlacLinkList AlacLinkList;
typedef struct AlacWriter AlacWriter;

typedef void (*AlacWakeupFn)(void *userdata);

/* Library information and errors. */

uint32_t alac_abi_version(void);
const char *alac_version(void);
/* Last error on this thread, or NULL. Valid until the next failing call on this thread. */
const char *alac_last_error(void);
void alac_string_free(char *value);

/* Options. */

enum {
    ALAC_OSC52_DISABLED = 0,
    ALAC_OSC52_ONLY_COPY = 1,
    ALAC_OSC52_ONLY_PASTE = 2,
    ALAC_OSC52_COPY_PASTE = 3,
};

enum {
    ALAC_CURSOR_BLOCK = 0,
    ALAC_CURSOR_UNDERLINE = 1,
    ALAC_CURSOR_BEAM = 2,
    ALAC_CURSOR_HOLLOW_BLOCK = 3,
    ALAC_CURSOR_HIDDEN = 4,
};

typedef struct {
    uint32_t columns;
    uint32_t rows;
    /* Cell size in pixels, used for Kitty graphics and text area size reports. */
    float cell_width;
    float cell_height;
    uint32_t scrollback;
    /* ALAC_OSC52_*, policy for OSC 52 and Kitty clipboard requests. */
    uint32_t osc52;
    /* ALAC_CURSOR_*, default cursor shape. */
    uint32_t cursor_shape;
    bool cursor_blinking;
    /* Allow applications to enable the Kitty keyboard protocol. */
    bool kitty_keyboard;
    /* Draw bold text with bright ANSI colors. */
    bool bold_is_bright;
    /* Clear the selection and scroll to the bottom when input is sent. */
    bool scroll_on_input;
    /* Deliver OSC 9 and OSC 99 notifications. */
    bool desktop_notifications;
    /* Characters ending semantic selection, or NULL for the default. */
    const char *semantic_escape_chars;
} AlacOptions;

/* Fill `out` with defaults: 80x24, 8x16 pixel cells and Kitty keyboard support. */
void alac_options_default(AlacOptions *out);

typedef struct {
    /* Program to run, or NULL for the user's login shell. */
    const char *program;
    const char *const *args;
    size_t arg_count;
    /* Startup directory, or NULL to inherit. */
    const char *working_directory;
    /* "KEY=VALUE" entries. TERM defaults to xterm-256color and COLORTERM to truecolor. */
    const char *const *env;
    size_t env_count;
    /* Read remaining output after the child exits. */
    bool drain_on_exit;
} AlacPtyOptions;

/* Lifecycle. */

/* Create a terminal whose child output is provided with alac_terminal_feed(). Bytes for the
 * child arrive as ALAC_EVENT_OUTPUT events. Useful for SSH, web sockets, tests and agents. */
AlacTerminal *alac_terminal_new_headless(const AlacOptions *options, AlacWakeupFn wakeup,
                                         void *userdata);
/* Spawn a child process on a new PTY. Returns NULL on failure; see alac_last_error(). */
AlacTerminal *alac_terminal_spawn(const AlacOptions *options, const AlacPtyOptions *pty,
                                  AlacWakeupFn wakeup, void *userdata);
/* Destroy a terminal, asking its PTY child to shut down. */
void alac_terminal_free(AlacTerminal *term);

bool alac_terminal_is_headless(const AlacTerminal *term);
/* Process id of the PTY child, or -1. */
int64_t alac_terminal_child_pid(const AlacTerminal *term);
/* Replace runtime options. Size fields are ignored; use alac_terminal_resize(). */
void alac_terminal_set_options(AlacTerminal *term, const AlacOptions *options);
void alac_terminal_resize(AlacTerminal *term, uint32_t columns, uint32_t rows, float cell_width,
                          float cell_height);
/* Focus controls focus reports, the hollow cursor and clipboard access. Default: focused. */
void alac_terminal_set_focused(AlacTerminal *term, bool focused);
/* Visibility controls notification policy. Default: visible. */
void alac_terminal_set_visible(AlacTerminal *term, bool visible);

/* Child I/O. */

/* Parse child output in a headless terminal. Returns false for PTY terminals. */
bool alac_terminal_feed(AlacTerminal *term, const uint8_t *data, size_t len);
/* Send raw bytes to the child. */
void alac_terminal_write(AlacTerminal *term, const uint8_t *data, size_t len);
/* Send committed text, such as IME output, as typed input. */
void alac_terminal_text(AlacTerminal *term, const char *text, size_t len);
/* Paste text, using bracketed paste when enabled by the application. */
void alac_terminal_paste(AlacTerminal *term, const char *text, size_t len);

/* Keyboard. */

enum {
    ALAC_KEY_PRESS = 0,
    ALAC_KEY_REPEAT = 1,
    ALAC_KEY_RELEASE = 2,
};

enum {
    ALAC_LOCATION_STANDARD = 0,
    ALAC_LOCATION_LEFT = 1,
    ALAC_LOCATION_RIGHT = 2,
    ALAC_LOCATION_NUMPAD = 3,
};

/* Modifier bits, matching the Kitty keyboard protocol. */
enum {
    ALAC_MOD_SHIFT = 1 << 0,
    ALAC_MOD_ALT = 1 << 1,
    ALAC_MOD_CONTROL = 1 << 2,
    ALAC_MOD_SUPER = 1 << 3,
};

enum {
    /* A printable key; set AlacKeyEvent.codepoint. */
    ALAC_KEY_CHARACTER = 0,
    ALAC_KEY_ENTER = 1,
    ALAC_KEY_TAB = 2,
    ALAC_KEY_BACKSPACE = 3,
    ALAC_KEY_ESCAPE = 4,
    ALAC_KEY_SPACE = 5,
    ALAC_KEY_INSERT = 6,
    ALAC_KEY_DELETE = 7,
    ALAC_KEY_HOME = 8,
    ALAC_KEY_END = 9,
    ALAC_KEY_PAGE_UP = 10,
    ALAC_KEY_PAGE_DOWN = 11,
    ALAC_KEY_UP = 12,
    ALAC_KEY_DOWN = 13,
    ALAC_KEY_LEFT = 14,
    ALAC_KEY_RIGHT = 15,
    ALAC_KEY_CAPS_LOCK = 16,
    ALAC_KEY_SCROLL_LOCK = 17,
    ALAC_KEY_NUM_LOCK = 18,
    ALAC_KEY_PRINT_SCREEN = 19,
    ALAC_KEY_PAUSE = 20,
    ALAC_KEY_CONTEXT_MENU = 21,
    ALAC_KEY_SHIFT = 22,
    ALAC_KEY_CONTROL = 23,
    ALAC_KEY_ALT = 24,
    ALAC_KEY_SUPER = 25,
    ALAC_KEY_HYPER = 26,
    ALAC_KEY_META = 27,
    ALAC_KEY_MEDIA_PLAY = 28,
    ALAC_KEY_MEDIA_PAUSE = 29,
    ALAC_KEY_MEDIA_PLAY_PAUSE = 30,
    ALAC_KEY_MEDIA_STOP = 31,
    ALAC_KEY_MEDIA_FAST_FORWARD = 32,
    ALAC_KEY_MEDIA_REWIND = 33,
    ALAC_KEY_MEDIA_TRACK_NEXT = 34,
    ALAC_KEY_MEDIA_TRACK_PREVIOUS = 35,
    ALAC_KEY_MEDIA_RECORD = 36,
    ALAC_KEY_VOLUME_DOWN = 37,
    ALAC_KEY_VOLUME_UP = 38,
    ALAC_KEY_VOLUME_MUTE = 39,
    /* F1 to F35 are ALAC_KEY_F1 + n - 1. */
    ALAC_KEY_F1 = 101,
};

#define ALAC_KEY_F(n) (ALAC_KEY_F1 + (n) - 1)

typedef struct {
    /* ALAC_KEY_PRESS, ALAC_KEY_REPEAT or ALAC_KEY_RELEASE. */
    uint32_t action;
    /* ALAC_KEY_* value. */
    uint32_t key;
    /* For ALAC_KEY_CHARACTER: the character with Shift and layout applied, ignoring Control and
     * Alt, such as 'A' for Shift+a or '!' for Shift+1. */
    uint32_t codepoint;
    /* The key's character without modifiers, such as '1' for '!', or 0 when unknown. */
    uint32_t unshifted_codepoint;
    /* UTF-8 text produced with all modifiers, such as "\x03" for Control+c, or NULL to derive
     * it. */
    const char *text;
    /* ALAC_LOCATION_* value. */
    uint32_t location;
    /* ALAC_MOD_* bits. */
    uint32_t mods;
    /* Alt/Option composed `text`, like macOS Option+f producing "ƒ", so no escape prefix is
     * sent for it. */
    bool alt_composes;
} AlacKeyEvent;

/* Encode and send a key using the legacy, xterm or Kitty keyboard protocol active in the
 * terminal. Returns whether the key was used; false means the host may handle it.
 *
 * In vi mode keys drive the vi cursor with the default bindings (hjkl, w/b/e, v/V/Ctrl-v,
 * y to copy, n/N to repeat a search, i to leave) and are never sent to the child. Keys like
 * '/' are left unhandled so the host can show search UI. */
bool alac_terminal_key(AlacTerminal *term, const AlacKeyEvent *event);
/* Encode a key for ALAC_MODE_* flags without a terminal, including default bindings such as
 * application cursor mode arrows. Writes at most cap - 1 bytes and a NUL terminator, returning
 * the full encoded length. */
size_t alac_encode_key(const AlacKeyEvent *event, uint32_t mode, uint8_t *out, size_t cap);

/* Mouse. Coordinates are viewport cells. */

enum {
    ALAC_MOUSE_LEFT = 0,
    ALAC_MOUSE_MIDDLE = 1,
    ALAC_MOUSE_RIGHT = 2,
};

enum {
    ALAC_WHEEL_IGNORED = 0,
    /* Reported to the application. */
    ALAC_WHEEL_REPORTED = 1,
    /* Sent as cursor keys for alternate screen applications. */
    ALAC_WHEEL_ALTERNATE_SCROLL = 2,
    /* Scrolled the viewport through history. */
    ALAC_WHEEL_SCROLLED = 3,
};

/* Returns true when reported to the application; otherwise the host may select text. Holding
 * Shift always bypasses mouse reporting. */
bool alac_terminal_mouse_button(AlacTerminal *term, uint32_t button, bool pressed,
                                uint32_t column, uint32_t row, uint32_t mods);
bool alac_terminal_mouse_move(AlacTerminal *term, uint32_t column, uint32_t row, uint32_t mods);
/* Whole lines and columns; positive values scroll up and left. Returns ALAC_WHEEL_*. */
uint32_t alac_terminal_mouse_wheel(AlacTerminal *term, int32_t lines, int32_t columns,
                                   uint32_t column, uint32_t row, uint32_t mods);

/* Events. */

enum {
    ALAC_EVENT_NONE = 0,
    /* New content is available; take a snapshot and redraw. */
    ALAC_EVENT_WAKEUP = 1,
    /* `data` is the new title, or NULL to restore the default title. */
    ALAC_EVENT_TITLE = 2,
    ALAC_EVENT_BELL = 3,
    ALAC_EVENT_CURSOR_BLINKING_CHANGE = 4,
    ALAC_EVENT_MOUSE_CURSOR_DIRTY = 5,
    /* OSC 7501 records changed; see alac_terminal_program_statuses(). */
    ALAC_EVENT_PROGRAM_STATUS_CHANGED = 6,
    /* OSC 9;4 progress changed; see `progress`. */
    ALAC_EVENT_PROGRESS_CHANGED = 7,
    /* Show `notification`. Report user actions with alac_terminal_notification_*(). */
    ALAC_EVENT_NOTIFICATION_SHOW = 8,
    /* Close the notification `serial`. */
    ALAC_EVENT_NOTIFICATION_CLOSE = 9,
    /* Answer with alac_terminal_notifications_alive(), passing `data` as the query. */
    ALAC_EVENT_NOTIFICATION_QUERY_ALIVE = 10,
    /* Headless terminals only: send `data` to the child. */
    ALAC_EVENT_OUTPUT = 11,
    /* The child exited with `exit_code`, or -1 when unknown. */
    ALAC_EVENT_CHILD_EXIT = 12,
    /* The terminal is shutting down. */
    ALAC_EVENT_EXIT = 13,
    /* OSC 7 working directory changed; read it with alac_terminal_working_directory(). */
    ALAC_EVENT_WORKING_DIRECTORY_CHANGED = 14,
};

enum {
    ALAC_PROGRESS_HIDDEN = 0,
    ALAC_PROGRESS_NORMAL = 1,
    ALAC_PROGRESS_ERROR = 2,
    ALAC_PROGRESS_INDETERMINATE = 3,
    ALAC_PROGRESS_PAUSED = 4,
};

typedef struct {
    uint32_t state;
    uint32_t percent;
} AlacProgress;

typedef struct {
    uint64_t serial;
    const char *id;
    const char *title;
    const char *body;
    const char *sound;
    const char *application;
    const char *const *buttons;
    size_t button_count;
    const char *const *icons;
    size_t icon_count;
    /* 0 low, 1 normal, 2 critical. */
    uint32_t urgency;
    /* Show 0 always, 1 when unfocused, 2 when invisible. */
    uint32_t occasion;
    /* Milliseconds until expiry, or -1. Expiry is handled by alac_terminal_tick(). */
    int64_t expires_in_ms;
    /* Activation should focus the terminal. */
    bool focus;
    bool report;
    bool report_close;
    bool persistent;
} AlacNotification;

typedef struct {
    uint32_t kind;
    int32_t exit_code;
    uint64_t serial;
    /* Output bytes, or NUL-terminated title and query text. */
    const uint8_t *data;
    size_t len;
    AlacProgress progress;
    const AlacNotification *notification;
} AlacEvent;

/* Take the next event, returning false when none are pending. Protocol replies, clipboard
 * requests and notification bookkeeping are serviced during this call, so drain events until
 * it returns false whenever the wakeup callback fires. */
bool alac_terminal_poll_event(AlacTerminal *term, AlacEvent *out);
/* Milliseconds until alac_terminal_tick() should run, or -1 without pending timers. Timers
 * cover notification expiry, the progress timeout, image animation and, for headless
 * terminals, synchronized update timeouts. */
int64_t alac_terminal_next_timeout_ms(const AlacTerminal *term);
void alac_terminal_tick(AlacTerminal *term);

/* The user activated a notification (button < 0) or one of its buttons. Returns true when the
 * host should focus this terminal. */
bool alac_terminal_notification_activated(AlacTerminal *term, uint64_t serial, int32_t button);
/* A notification was dismissed or failed to show. */
void alac_terminal_notification_closed(AlacTerminal *term, uint64_t serial);
void alac_terminal_notifications_alive(AlacTerminal *term, const char *query,
                                       const uint64_t *serials, size_t count);

/* Colors. */

#define ALAC_COLOR_COUNT 269
/* 0-15 ANSI, 16-231 color cube, 232-255 grayscale, then the special colors below. */
#define ALAC_COLOR_FOREGROUND 256
#define ALAC_COLOR_BACKGROUND 257
#define ALAC_COLOR_CURSOR 258
#define ALAC_COLOR_DIM_BLACK 259
#define ALAC_COLOR_BRIGHT_FOREGROUND 267
#define ALAC_COLOR_DIM_FOREGROUND 268

void alac_terminal_set_color(AlacTerminal *term, uint32_t index, uint32_t rgb);
/* Set `count` colors starting at index 0. */
void alac_terminal_set_colors(AlacTerminal *term, const uint32_t *colors, size_t count);
/* Set foreground and background, deriving cursor, dim and bright colors. */
void alac_terminal_set_primary_colors(AlacTerminal *term, uint32_t foreground,
                                      uint32_t background);
/* Effective color, including application overrides from OSC 4/10/11/12. */
uint32_t alac_terminal_color(const AlacTerminal *term, uint32_t index);

/* Clipboard. The library enforces focus and the osc52 policy before calling back. */

enum {
    ALAC_CLIPBOARD = 0,
    ALAC_CLIPBOARD_PRIMARY = 1,
};

typedef struct {
    const char *mime_type;
    const uint8_t *data;
    size_t len;
} AlacClipboardItem;

typedef struct {
    void *userdata;
    /* Add available MIME types with alac_clipboard_sink_add_format(). Return false when the
     * location is unsupported. */
    bool (*formats)(void *userdata, uint32_t location, AlacClipboardSink *sink);
    /* Provide one MIME type with alac_clipboard_sink_set_data(). Return false when missing. */
    bool (*read)(void *userdata, uint32_t location, const char *mime_type,
                 AlacClipboardSink *sink);
    /* Replace the clipboard with all items. OSC 52 writes a single text/plain item. */
    bool (*write)(void *userdata, uint32_t location, const AlacClipboardItem *items,
                  size_t count);
} AlacClipboardCallbacks;

void alac_clipboard_sink_add_format(AlacClipboardSink *sink, const char *mime_type);
void alac_clipboard_sink_set_data(AlacClipboardSink *sink, const uint8_t *data, size_t len);
/* Install callbacks for OSC 52, Kitty clipboard (OSC 5522) and paste; NULL removes them.
 * Callbacks run on the thread polling events or pasting. */
void alac_terminal_set_clipboard(AlacTerminal *term, const AlacClipboardCallbacks *callbacks);
/* Paste from the clipboard callbacks, sending Kitty paste events when the application asked
 * for them. Returns false without clipboard callbacks. */
bool alac_terminal_paste_clipboard(AlacTerminal *term, uint32_t location);

/* Rendering snapshots. */

/* Raw cell attributes. */
enum {
    ALAC_CELL_INVERSE = 1 << 0,
    ALAC_CELL_BOLD = 1 << 1,
    ALAC_CELL_ITALIC = 1 << 2,
    ALAC_CELL_UNDERLINE = 1 << 3,
    ALAC_CELL_WRAPLINE = 1 << 4,
    ALAC_CELL_WIDE_CHAR = 1 << 5,
    ALAC_CELL_WIDE_CHAR_SPACER = 1 << 6,
    ALAC_CELL_DIM = 1 << 7,
    ALAC_CELL_HIDDEN = 1 << 8,
    ALAC_CELL_STRIKEOUT = 1 << 9,
    ALAC_CELL_LEADING_WIDE_CHAR_SPACER = 1 << 10,
    ALAC_CELL_DOUBLE_UNDERLINE = 1 << 11,
    ALAC_CELL_UNDERCURL = 1 << 12,
    ALAC_CELL_DOTTED_UNDERLINE = 1 << 13,
    ALAC_CELL_DASHED_UNDERLINE = 1 << 14,
    /* Part of the selection. */
    ALAC_CELL_SELECTED = 1 << 16,
    /* Uses the default background and may be drawn transparent. */
    ALAC_CELL_DEFAULT_BACKGROUND = 1 << 17,
    /* Has an explicit SGR 58 underline color. */
    ALAC_CELL_UNDERLINE_COLOR = 1 << 18,
    /* Kitty Unicode placeholder; its image is drawn from the snapshot images. */
    ALAC_CELL_IMAGE_PLACEHOLDER = 1 << 19,
};

typedef struct {
    uint32_t codepoint;
    /* Colors after bold, dim, inverse and hidden attributes. */
    uint32_t fg;
    uint32_t bg;
    uint32_t underline_color;
    /* ALAC_CELL_* bits. */
    uint32_t flags;
    /* Combining characters in AlacSnapshotInfo.codepoints. */
    uint32_t zerowidth_index;
    uint16_t zerowidth_count;
    uint16_t reserved;
    /* One-based hyperlink index, or 0. */
    uint32_t hyperlink;
} AlacCell;

typedef struct {
    uint32_t column;
    uint32_t row;
    /* ALAC_CURSOR_*. Unfocused terminals report a hollow block. */
    uint32_t shape;
    uint32_t color;
    /* Color for text drawn over a block cursor. */
    uint32_t text_color;
    bool blinking;
    /* The cursor covers a double-width character. */
    bool wide;
} AlacCursor;

/* Damaged columns left..=right of a viewport row. */
typedef struct {
    uint32_t row;
    uint32_t left;
    uint32_t right;
} AlacLineDamage;

/* A visible Kitty graphics placement. Draw the src rectangle of the image into the dest
 * rectangle, in pixels relative to the top-left of the cell grid, ordered by z_index. Negative
 * z_index values are drawn below text. */
typedef struct {
    uint32_t image_id;
    uint32_t placement_id;
    /* Changes whenever the pixels change, for texture caching. */
    uint64_t generation;
    uint32_t width;
    uint32_t height;
    /* Straight-alpha RGBA pixels, or NULL when only `png` is available. */
    const uint8_t *rgba;
    size_t rgba_len;
    const uint8_t *png;
    size_t png_len;
    int32_t z_index;
    float dest_x;
    float dest_y;
    float dest_width;
    float dest_height;
    float src_x;
    float src_y;
    float src_width;
    float src_height;
} AlacImage;

typedef struct {
    uint32_t columns;
    uint32_t rows;
    /* Lines scrolled into history. */
    uint32_t display_offset;
    uint32_t history_size;
    /* ALAC_MODE_* flags. */
    uint32_t mode;
    uint32_t foreground;
    uint32_t background;
    /* Redraw everything; otherwise only `damage` changed since the previous snapshot. */
    bool full_damage;
    AlacCursor cursor;
    /* rows * columns cells in row-major order. */
    const AlacCell *cells;
    const uint32_t *codepoints;
    size_t codepoint_count;
    const AlacLineDamage *damage;
    size_t damage_count;
    const AlacImage *images;
    size_t image_count;
} AlacSnapshotInfo;

/* Capture visible content. Damage is relative to the previous snapshot. */
AlacSnapshot *alac_terminal_snapshot(AlacTerminal *term);
/* Reusable snapshot: create once with alac_snapshot_new() and refresh it every frame with
 * alac_terminal_snapshot_update(), which only copies damaged rows when the snapshot was the
 * last one updated for this terminal. Updating invalidates pointers from alac_snapshot_info(). */
AlacSnapshot *alac_snapshot_new(void);
bool alac_terminal_snapshot_update(AlacTerminal *term, AlacSnapshot *snapshot);
void alac_snapshot_free(AlacSnapshot *snapshot);
void alac_snapshot_info(const AlacSnapshot *snapshot, AlacSnapshotInfo *out);
/* UTF-8 text of a cell with combining characters. Writes at most cap - 1 bytes and a NUL,
 * returning the full length. */
size_t alac_snapshot_cell_text(const AlacSnapshot *snapshot, uint32_t column, uint32_t row,
                               char *out, size_t cap);
const char *alac_snapshot_hyperlink_uri(const AlacSnapshot *snapshot, uint32_t index);
/* Cells of the same link share an identifier, for hover highlighting. */
const char *alac_snapshot_hyperlink_id(const AlacSnapshot *snapshot, uint32_t index);

/* Terminal modes. */

enum {
    ALAC_MODE_SHOW_CURSOR = 1 << 0,
    ALAC_MODE_APP_CURSOR = 1 << 1,
    ALAC_MODE_APP_KEYPAD = 1 << 2,
    ALAC_MODE_MOUSE_REPORT_CLICK = 1 << 3,
    ALAC_MODE_BRACKETED_PASTE = 1 << 4,
    ALAC_MODE_SGR_MOUSE = 1 << 5,
    ALAC_MODE_MOUSE_MOTION = 1 << 6,
    ALAC_MODE_LINE_WRAP = 1 << 7,
    ALAC_MODE_LINE_FEED_NEW_LINE = 1 << 8,
    ALAC_MODE_ORIGIN = 1 << 9,
    ALAC_MODE_INSERT = 1 << 10,
    ALAC_MODE_FOCUS_IN_OUT = 1 << 11,
    ALAC_MODE_ALT_SCREEN = 1 << 12,
    ALAC_MODE_MOUSE_DRAG = 1 << 13,
    ALAC_MODE_UTF8_MOUSE = 1 << 14,
    ALAC_MODE_ALTERNATE_SCROLL = 1 << 15,
    ALAC_MODE_VI = 1 << 16,
    ALAC_MODE_URGENCY_HINTS = 1 << 17,
    ALAC_MODE_DISAMBIGUATE_ESC_CODES = 1 << 18,
    ALAC_MODE_REPORT_EVENT_TYPES = 1 << 19,
    ALAC_MODE_REPORT_ALTERNATE_KEYS = 1 << 20,
    ALAC_MODE_REPORT_ALL_KEYS_AS_ESC = 1 << 21,
    ALAC_MODE_REPORT_ASSOCIATED_TEXT = 1 << 22,
    ALAC_MODE_CLIPBOARD_PASTE_EVENTS = 1 << 23,
};

uint32_t alac_terminal_mode(const AlacTerminal *term);

/* Scrolling and text. Grid lines are negative in history; 0 is the top of the screen. */

enum {
    ALAC_SCROLL_DELTA = 0,
    ALAC_SCROLL_PAGE_UP = 1,
    ALAC_SCROLL_PAGE_DOWN = 2,
    ALAC_SCROLL_TOP = 3,
    ALAC_SCROLL_BOTTOM = 4,
};

/* Scroll the viewport; positive deltas move up into history. */
void alac_terminal_scroll(AlacTerminal *term, uint32_t kind, int32_t delta);
/* Scroll to the previous (direction < 0) or next OSC 133 shell prompt. */
void alac_terminal_jump_to_prompt(AlacTerminal *term, int32_t direction);
uint32_t alac_terminal_history_size(const AlacTerminal *term);
char *alac_terminal_screen_text(const AlacTerminal *term);
char *alac_terminal_text_range(const AlacTerminal *term, int32_t start_line, int32_t end_line);

/* Selection. */

enum {
    ALAC_SELECTION_SIMPLE = 0,
    ALAC_SELECTION_BLOCK = 1,
    ALAC_SELECTION_SEMANTIC = 2,
    ALAC_SELECTION_LINES = 3,
};

enum {
    ALAC_SIDE_LEFT = 0,
    ALAC_SIDE_RIGHT = 1,
};

/* `side` is the half of the cell under the pointer. */
void alac_terminal_select_start(AlacTerminal *term, uint32_t kind, uint32_t column, uint32_t row,
                                uint32_t side);
void alac_terminal_select_update(AlacTerminal *term, uint32_t column, uint32_t row,
                                 uint32_t side);
/* Start a selection for a click: 1 click selects cells (a rectangle when `block` is set, such
 * as with Control or Option held), 2 clicks a word and 3 clicks whole lines. */
void alac_terminal_select_click(AlacTerminal *term, uint32_t column, uint32_t row, uint32_t side,
                                uint32_t clicks, bool block);
/* Viewport cell and ALAC_SIDE_* under a pixel position relative to the grid's top-left. Any
 * output may be NULL. */
void alac_terminal_cell_at(const AlacTerminal *term, float x, float y, uint32_t *column,
                           uint32_t *row, uint32_t *side);
/* Select the entire buffer including history. */
void alac_terminal_select_all(AlacTerminal *term);
void alac_terminal_select_clear(AlacTerminal *term);
/* Selected text, or NULL. */
char *alac_terminal_selection_text(const AlacTerminal *term);
/* Copy the selection to ALAC_CLIPBOARD or ALAC_CLIPBOARD_PRIMARY through the clipboard
 * callbacks. Returns false without a selection or callbacks, or when the write failed. */
bool alac_terminal_copy_selection(AlacTerminal *term, uint32_t location);

/* Search. */

typedef struct {
    int32_t start_line;
    uint32_t start_column;
    int32_t end_line;
    uint32_t end_column;
} AlacMatch;

/* Find the next regex match toward the bottom (direction >= 0) or top, wrapping around and
 * continuing from the previous match. The match is selected and scrolled into view. Returns 1
 * when found, 0 when not found and -1 for invalid patterns. */
int32_t alac_terminal_search(AlacTerminal *term, const char *pattern, int32_t direction,
                             AlacMatch *out);
void alac_terminal_search_clear(AlacTerminal *term);

/* Links: OSC 8 hyperlinks and URLs detected in text. */

/* URI of the link at a viewport cell, or NULL. `range` (optional) receives its grid range and
 * `hyperlink` (optional) whether it's an OSC 8 hyperlink rather than detected text. */
char *alac_terminal_link_at(AlacTerminal *term, uint32_t column, uint32_t row, AlacMatch *range,
                            bool *hyperlink);
/* Regex for URL detection. NULL restores the default; "" disables detection. Returns false for
 * invalid patterns. */
bool alac_terminal_set_url_pattern(AlacTerminal *term, const char *pattern);

typedef struct {
    const char *uri;
    AlacMatch range;
    bool hyperlink;
} AlacLink;

/* Visible links in reading order, for keyboard hint interfaces. */
AlacLinkList *alac_terminal_visible_links(AlacTerminal *term);
size_t alac_link_list_len(const AlacLinkList *list);
/* Link at `index`, valid until the list is freed. */
const AlacLink *alac_link_list_get(const AlacLinkList *list, size_t index);
void alac_link_list_free(AlacLinkList *list);

/* Vi mode: a keyboard cursor for moving through history and selecting text. Its position is the
 * snapshot cursor while active (ALAC_MODE_VI). */

enum {
    ALAC_VI_UP = 0,
    ALAC_VI_DOWN = 1,
    ALAC_VI_LEFT = 2,
    ALAC_VI_RIGHT = 3,
    ALAC_VI_FIRST = 4,
    ALAC_VI_LAST = 5,
    ALAC_VI_FIRST_OCCUPIED = 6,
    ALAC_VI_HIGH = 7,
    ALAC_VI_MIDDLE = 8,
    ALAC_VI_LOW = 9,
    ALAC_VI_SEMANTIC_LEFT = 10,
    ALAC_VI_SEMANTIC_RIGHT = 11,
    ALAC_VI_SEMANTIC_LEFT_END = 12,
    ALAC_VI_SEMANTIC_RIGHT_END = 13,
    ALAC_VI_WORD_LEFT = 14,
    ALAC_VI_WORD_RIGHT = 15,
    ALAC_VI_WORD_LEFT_END = 16,
    ALAC_VI_WORD_RIGHT_END = 17,
    ALAC_VI_BRACKET = 18,
    ALAC_VI_PARAGRAPH_UP = 19,
    ALAC_VI_PARAGRAPH_DOWN = 20,
};

void alac_terminal_vi_toggle(AlacTerminal *term);
/* Move the vi cursor by an ALAC_VI_* motion. Ignored outside vi mode. */
void alac_terminal_vi_motion(AlacTerminal *term, uint32_t motion);
/* Toggle an ALAC_SELECTION_* selection at the vi cursor, like vi's v, V and Ctrl-v. */
void alac_terminal_vi_select(AlacTerminal *term, uint32_t kind);

/* Thread-safe writing. */

/* Writer usable from any thread, also after the terminal is freed. Bytes written to a headless
 * terminal arrive as ALAC_EVENT_OUTPUT events. */
AlacWriter *alac_terminal_writer(const AlacTerminal *term);
/* Queue bytes for the child process. Returns false once the terminal is gone. */
bool alac_writer_write(const AlacWriter *writer, const uint8_t *data, size_t len);
void alac_writer_free(AlacWriter *writer);

/* Protocol state. */

void alac_terminal_progress(const AlacTerminal *term, AlacProgress *out);
/* Window title set by the application, or NULL. */
char *alac_terminal_title(const AlacTerminal *term);
/* OSC 7 working directory path, or NULL. When `host` is non-NULL it receives the reported host
 * name, which is not a local path when it names another machine. */
char *alac_terminal_working_directory(const AlacTerminal *term, char **host);

enum {
    ALAC_SHELL_UNKNOWN = 0,
    ALAC_SHELL_PROMPT = 1,
    ALAC_SHELL_INPUT = 2,
    ALAC_SHELL_OUTPUT = 3,
};

typedef struct {
    /* ALAC_SHELL_* phase. */
    uint32_t phase;
    bool has_last_command;
    uint64_t last_command_id;
    /* Exit status of the last command, or -1 when unknown. */
    int32_t last_exit_status;
} AlacShellState;

/* OSC 133 shell integration. */
void alac_terminal_shell_state(const AlacTerminal *term, AlacShellState *out);
char *alac_terminal_last_command(const AlacTerminal *term);
char *alac_terminal_last_command_output(const AlacTerminal *term);

enum {
    ALAC_STATUS_IDLE = 0,
    ALAC_STATUS_WORKING = 1,
    ALAC_STATUS_DONE = 2,
    ALAC_STATUS_BLOCKED = 3,
    ALAC_STATUS_ERROR = 4,
};

enum {
    ALAC_BLOCKED_NONE = 0,
    ALAC_BLOCKED_PERMISSION = 1,
    ALAC_BLOCKED_QUESTION = 2,
    ALAC_BLOCKED_AUTH = 3,
};

typedef struct {
    /* Empty for the root record; child ids are slash-separated paths. */
    const char *id;
    /* Application name, resolved through parent records, or NULL. */
    const char *app;
    const char *title;
    const char *message;
    uint32_t state;
    uint32_t blocked_kind;
    /* Percent, or -1. */
    int32_t progress;
} AlacProgramStatus;

/* OSC 7501 program status records, oldest first. */
AlacProgramStatusList *alac_terminal_program_statuses(const AlacTerminal *term);
size_t alac_program_status_list_len(const AlacProgramStatusList *list);
const AlacProgramStatus *alac_program_status_list_get(const AlacProgramStatusList *list,
                                                      size_t index);
void alac_program_status_list_free(AlacProgramStatusList *list);

#ifdef __cplusplus
}
#endif

#endif
