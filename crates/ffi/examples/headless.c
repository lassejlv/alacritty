/*
 * Headless terminal: feed child output, service events and inspect a snapshot.
 *
 * Exits with a non-zero status when a check fails, so it doubles as an ABI test.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "alacritty.h"

#define CHECK(condition)                                                     \
    do {                                                                     \
        if (!(condition)) {                                                  \
            fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, \
                    #condition);                                             \
            exit(1);                                                         \
        }                                                                    \
    } while (0)

static char clipboard[256];

static bool clipboard_formats(void *userdata, uint32_t location, AlacClipboardSink *sink) {
    (void)userdata;
    (void)location;
    alac_clipboard_sink_add_format(sink, "text/plain");
    return true;
}

static bool clipboard_read(void *userdata, uint32_t location, const char *mime_type,
                           AlacClipboardSink *sink) {
    (void)userdata;
    (void)location;
    if (strcmp(mime_type, "text/plain") != 0) return false;
    alac_clipboard_sink_set_data(sink, (const uint8_t *)clipboard, strlen(clipboard));
    return true;
}

static bool clipboard_write(void *userdata, uint32_t location, const AlacClipboardItem *items,
                            size_t count) {
    (void)userdata;
    (void)location;
    if (count == 0 || items[0].len >= sizeof(clipboard)) return false;
    memcpy(clipboard, items[0].data, items[0].len);
    clipboard[items[0].len] = '\0';
    return true;
}

static void feed(AlacTerminal *term, const char *text) {
    CHECK(alac_terminal_feed(term, (const uint8_t *)text, strlen(text)));
}

/* Drain events, returning the bytes the terminal wants to send to the child. */
static size_t drain(AlacTerminal *term, char *output, size_t cap, char *title, size_t title_cap) {
    size_t len = 0;
    AlacEvent event;
    while (alac_terminal_poll_event(term, &event)) {
        switch (event.kind) {
            case ALAC_EVENT_OUTPUT:
                if (len + event.len < cap) {
                    memcpy(output + len, event.data, event.len);
                    len += event.len;
                }
                break;
            case ALAC_EVENT_TITLE:
                if (title && event.data) snprintf(title, title_cap, "%s", (const char *)event.data);
                break;
            case ALAC_EVENT_NOTIFICATION_SHOW:
                printf("notification: %s\n", event.notification->title);
                break;
            default:
                break;
        }
    }
    output[len] = '\0';
    return len;
}

int main(void) {
    CHECK(alac_abi_version() == ALAC_ABI_VERSION);

    AlacOptions options;
    alac_options_default(&options);
    options.columns = 40;
    options.rows = 10;
    options.osc52 = ALAC_OSC52_COPY_PASTE;

    AlacTerminal *term = alac_terminal_new_headless(&options, NULL, NULL);
    CHECK(term != NULL);
    CHECK(alac_terminal_is_headless(term));

    AlacClipboardCallbacks callbacks = {
        .userdata = NULL,
        .formats = clipboard_formats,
        .read = clipboard_read,
        .write = clipboard_write,
    };
    alac_terminal_set_clipboard(term, &callbacks);

    char output[1024];
    char title[64] = "";

    /* Title, colors, OSC 52 copy and a cursor position query. */
    feed(term, "\x1b]2;demo\x07hello \x1b[1;32mworld\x1b[0m\x1b]52;c;Y29waWVk\x07\x1b[6n");
    drain(term, output, sizeof(output), title, sizeof(title));
    CHECK(strcmp(title, "demo") == 0);
    CHECK(strcmp(clipboard, "copied") == 0);
    CHECK(strcmp(output, "\x1b[1;12R") == 0);

    /* Keys are encoded for the active keyboard protocol. */
    AlacKeyEvent key = {.action = ALAC_KEY_PRESS, .key = ALAC_KEY_CHARACTER, .codepoint = 'c',
                        .mods = ALAC_MOD_CONTROL};
    CHECK(alac_terminal_key(term, &key));
    drain(term, output, sizeof(output), NULL, 0);
    CHECK(strcmp(output, "\x03") == 0);

    feed(term, "\x1b[>1u");
    CHECK(alac_terminal_mode(term) & ALAC_MODE_DISAMBIGUATE_ESC_CODES);
    CHECK(alac_terminal_key(term, &key));
    drain(term, output, sizeof(output), NULL, 0);
    CHECK(strcmp(output, "\x1b[99;5u") == 0);

    uint8_t encoded[32];
    AlacKeyEvent up = {.action = ALAC_KEY_PRESS, .key = ALAC_KEY_UP};
    CHECK(alac_encode_key(&up, ALAC_MODE_APP_CURSOR, encoded, sizeof(encoded)) == 3);
    CHECK(memcmp(encoded, "\x1bOA", 3) == 0);

    /* A 1x1 Kitty graphics image and an OSC 9 notification. */
    feed(term, "\r\n\x1b_Ga=T,f=32,s=1,v=1,i=3;AP8A/w==\x1b\\\x1b]9;done\x07");
    drain(term, output, sizeof(output), NULL, 0);

    AlacSnapshot *snapshot = alac_terminal_snapshot(term);
    CHECK(snapshot != NULL);
    AlacSnapshotInfo info;
    alac_snapshot_info(snapshot, &info);
    CHECK(info.columns == 40 && info.rows == 10);
    CHECK(info.cells[0].codepoint == 'h');
    CHECK(info.cells[6].flags & ALAC_CELL_BOLD);
    CHECK(info.cells[6].fg == 0xaac474 || info.cells[6].fg == 0x90a959);
    CHECK(info.image_count == 1);
    CHECK(info.images[0].image_id == 3 && info.images[0].rgba_len == 4);
    CHECK(info.images[0].rgba[1] == 0xff);

    char text[8];
    CHECK(alac_snapshot_cell_text(snapshot, 1, 0, text, sizeof(text)) == 1);
    CHECK(strcmp(text, "e") == 0);
    alac_snapshot_free(snapshot);

    char *screen = alac_terminal_screen_text(term);
    printf("%s\n", screen);
    CHECK(strncmp(screen, "hello world", 11) == 0);
    alac_string_free(screen);

    AlacMatch match;
    CHECK(alac_terminal_search(term, "wor.d", 1, &match) == 1);
    CHECK(match.start_line == 0 && match.start_column == 6 && match.end_column == 10);
    char *selection = alac_terminal_selection_text(term);
    CHECK(strcmp(selection, "world") == 0);
    alac_string_free(selection);
    CHECK(alac_terminal_search(term, "(", 1, &match) == -1);
    CHECK(alac_last_error() != NULL);

    /* Reusable snapshots only copy damaged rows. */
    AlacSnapshot *frame = alac_snapshot_new();
    CHECK(alac_terminal_snapshot_update(term, frame));
    alac_terminal_search_clear(term);
    alac_terminal_select_clear(term);
    CHECK(alac_terminal_snapshot_update(term, frame));
    feed(term, "\x1b[8;1Hsee https://example.com now");
    CHECK(alac_terminal_snapshot_update(term, frame));
    alac_snapshot_info(frame, &info);
    CHECK(!info.full_damage && info.damage_count >= 1 && info.damage_count < info.rows);
    CHECK(info.cells[7 * 40 + 4].codepoint == 'h');
    CHECK(info.cursor.row == 7 && info.cursor.text_color != info.cursor.color);
    alac_snapshot_free(frame);

    /* Links. */
    bool hyperlink = true;
    char *uri = alac_terminal_link_at(term, 10, 7, &match, &hyperlink);
    CHECK(uri != NULL && strcmp(uri, "https://example.com") == 0 && !hyperlink);
    CHECK(match.start_column == 4 && match.end_column == 22);
    alac_string_free(uri);
    AlacLinkList *links = alac_terminal_visible_links(term);
    CHECK(alac_link_list_len(links) == 1);
    CHECK(strcmp(alac_link_list_get(links, 0)->uri, "https://example.com") == 0);
    alac_link_list_free(links);

    /* Click selection and copy. */
    uint32_t column, row, side;
    alac_terminal_cell_at(term, 8 * 25 + 1, 16 * 7 + 1, &column, &row, &side);
    CHECK(column == 25 && row == 7 && side == ALAC_SIDE_LEFT);
    alac_terminal_select_click(term, column, row, side, 2, false);
    CHECK(alac_terminal_copy_selection(term, ALAC_CLIPBOARD));
    CHECK(strcmp(clipboard, "now") == 0);

    /* Vi mode keys never reach the child. */
    alac_terminal_vi_toggle(term);
    CHECK(alac_terminal_mode(term) & ALAC_MODE_VI);
    alac_terminal_vi_motion(term, ALAC_VI_FIRST);
    alac_terminal_vi_select(term, ALAC_SELECTION_SIMPLE);
    alac_terminal_vi_motion(term, ALAC_VI_WORD_RIGHT_END);
    selection = alac_terminal_selection_text(term);
    CHECK(strcmp(selection, "see") == 0);
    alac_string_free(selection);
    AlacKeyEvent i_key = {.action = ALAC_KEY_PRESS, .key = ALAC_KEY_CHARACTER, .codepoint = 'i'};
    CHECK(alac_terminal_key(term, &i_key));
    CHECK(!(alac_terminal_mode(term) & ALAC_MODE_VI));
    CHECK(drain(term, output, sizeof(output), NULL, 0) == 0);

    /* Writers work from any thread. */
    AlacWriter *writer = alac_terminal_writer(term);
    CHECK(alac_writer_write(writer, (const uint8_t *)"queued", 6));
    CHECK(drain(term, output, sizeof(output), NULL, 0) == 6);
    CHECK(memcmp(output, "queued", 6) == 0);

    char *current_title = alac_terminal_title(term);
    CHECK(current_title != NULL);
    alac_string_free(current_title);

    alac_terminal_free(term);
    CHECK(!alac_writer_write(writer, (const uint8_t *)"x", 1));
    alac_writer_free(writer);
    printf("ok\n");
    return 0;
}
