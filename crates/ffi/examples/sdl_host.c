/*
 * A small graphical terminal: SDL2 for the window and input, SDL2_ttf for glyphs.
 *
 *   cc sdl_host.c $(pkg-config --cflags --libs sdl2 SDL2_ttf alacritty_ffi) -o sdl_host
 *   ./sdl_host [--font PATH] [--size PT] [--command CMD] [--exit-after MS] [--screenshot BMP]
 *
 * The font defaults to $ALACRITTY_FONT, then Menlo on macOS or DejaVu Sans Mono elsewhere.
 * Selection uses the left mouse button (double and triple click for words and lines), Shift
 * bypasses mouse reporting, and Ctrl+Shift+C/V (Cmd+C/V on macOS) copy and paste.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <SDL.h>
#include <SDL_ttf.h>

#include "alacritty.h"

#ifdef __APPLE__
#define DEFAULT_FONT "/System/Library/Fonts/Menlo.ttc"
#define COPY_MODS KMOD_GUI
#else
#define DEFAULT_FONT "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
#define COPY_MODS (KMOD_CTRL | KMOD_SHIFT)
#endif

#define GLYPH_CACHE 4096
#define ALL_UNDERLINES                                                                    \
    (ALAC_CELL_UNDERLINE | ALAC_CELL_DOUBLE_UNDERLINE | ALAC_CELL_UNDERCURL |             \
     ALAC_CELL_DOTTED_UNDERLINE | ALAC_CELL_DASHED_UNDERLINE)

typedef struct {
    uint32_t key;
    SDL_Texture *texture;
    int width;
    int height;
} Glyph;

typedef struct {
    SDL_Window *window;
    SDL_Renderer *renderer;
    TTF_Font *font;
    AlacTerminal *term;
    AlacSnapshot *snapshot;
    Glyph glyphs[GLYPH_CACHE];
    int cell_width;
    int cell_height;
    uint32_t wakeup_event;
    bool selecting;
    bool running;
} Host;

/* Runs on any thread: only post an event for the main loop. */
static void wakeup(void *userdata) {
    Host *host = userdata;
    SDL_Event event = {.type = host->wakeup_event};
    SDL_PushEvent(&event);
}

static bool clipboard_formats(void *userdata, uint32_t location, AlacClipboardSink *sink) {
    (void)userdata;
    if (location != ALAC_CLIPBOARD) return false;
    if (SDL_HasClipboardText()) alac_clipboard_sink_add_format(sink, "text/plain");
    return true;
}

static bool clipboard_read(void *userdata, uint32_t location, const char *mime_type,
                           AlacClipboardSink *sink) {
    (void)userdata;
    if (location != ALAC_CLIPBOARD || strcmp(mime_type, "text/plain") != 0) return false;
    char *text = SDL_GetClipboardText();
    alac_clipboard_sink_set_data(sink, (const uint8_t *)text, strlen(text));
    SDL_free(text);
    return true;
}

static bool clipboard_write(void *userdata, uint32_t location, const AlacClipboardItem *items,
                            size_t count) {
    (void)userdata;
    if (location != ALAC_CLIPBOARD) return false;
    for (size_t i = 0; i < count; i++) {
        if (strcmp(items[i].mime_type, "text/plain") != 0) continue;
        char *text = SDL_malloc(items[i].len + 1);
        if (!text) return false;
        memcpy(text, items[i].data, items[i].len);
        text[items[i].len] = '\0';
        bool ok = SDL_SetClipboardText(text) == 0;
        SDL_free(text);
        return ok;
    }
    return false;
}

/* White glyph texture, tinted per cell with SDL_SetTextureColorMod. */
static Glyph *glyph(Host *host, uint32_t codepoint, bool bold, bool italic) {
    uint32_t key = codepoint | (bold ? 1u << 30 : 0) | (italic ? 1u << 29 : 0);
    Glyph *slot = &host->glyphs[(key * 2654435761u) % GLYPH_CACHE];
    if (slot->texture && slot->key == key) return slot;
    if (slot->texture) SDL_DestroyTexture(slot->texture);
    slot->texture = NULL;

    TTF_SetFontStyle(host->font, (bold ? TTF_STYLE_BOLD : 0) | (italic ? TTF_STYLE_ITALIC : 0));
    SDL_Color white = {255, 255, 255, 255};
    SDL_Surface *surface = TTF_RenderGlyph32_Blended(host->font, codepoint, white);
    if (!surface) return NULL;
    slot->texture = SDL_CreateTextureFromSurface(host->renderer, surface);
    slot->width = surface->w;
    slot->height = surface->h;
    slot->key = key;
    SDL_FreeSurface(surface);
    return slot->texture ? slot : NULL;
}

static void set_color(SDL_Renderer *renderer, uint32_t rgb) {
    SDL_SetRenderDrawColor(renderer, rgb >> 16, (rgb >> 8) & 0xff, rgb & 0xff, 255);
}

static void draw_glyph(Host *host, const AlacCell *cell, uint32_t fg, int x, int y) {
    if (cell->codepoint <= ' ' || (cell->flags & ALAC_CELL_IMAGE_PLACEHOLDER)) return;
    Glyph *g = glyph(host, cell->codepoint, cell->flags & ALAC_CELL_BOLD,
                     cell->flags & ALAC_CELL_ITALIC);
    if (!g) return;
    SDL_SetTextureColorMod(g->texture, fg >> 16, (fg >> 8) & 0xff, fg & 0xff);
    SDL_Rect dest = {x, y, g->width, g->height};
    SDL_RenderCopy(host->renderer, g->texture, NULL, &dest);
}

static void draw(Host *host) {
    alac_terminal_snapshot_update(host->term, host->snapshot);
    AlacSnapshotInfo info;
    alac_snapshot_info(host->snapshot, &info);
    int cw = host->cell_width, ch = host->cell_height;

    set_color(host->renderer, info.background);
    SDL_RenderClear(host->renderer);

    for (uint32_t row = 0; row < info.rows; row++) {
        for (uint32_t column = 0; column < info.columns; column++) {
            const AlacCell *cell = &info.cells[row * info.columns + column];
            if (cell->flags & ALAC_CELL_WIDE_CHAR_SPACER) continue;
            int width = (cell->flags & ALAC_CELL_WIDE_CHAR) ? 2 * cw : cw;
            uint32_t fg = cell->fg, bg = cell->bg;
            if (cell->flags & ALAC_CELL_SELECTED) {
                uint32_t swap = fg;
                fg = bg;
                bg = swap;
            }
            if (!(cell->flags & ALAC_CELL_DEFAULT_BACKGROUND) || (cell->flags & ALAC_CELL_SELECTED)) {
                set_color(host->renderer, bg);
                SDL_RenderFillRect(host->renderer, &(SDL_Rect){column * cw, row * ch, width, ch});
            }
            draw_glyph(host, cell, fg, column * cw, row * ch);
            if (cell->flags & ALL_UNDERLINES) {
                set_color(host->renderer, cell->underline_color);
                SDL_RenderFillRect(host->renderer,
                                   &(SDL_Rect){column * cw, row * ch + ch - 2, width, 1});
            }
            if (cell->flags & ALAC_CELL_STRIKEOUT) {
                set_color(host->renderer, fg);
                SDL_RenderFillRect(host->renderer,
                                   &(SDL_Rect){column * cw, row * ch + ch / 2, width, 1});
            }
        }
    }

    for (size_t i = 0; i < info.image_count; i++) {
        const AlacImage *image = &info.images[i];
        if (!image->rgba) continue;
        SDL_Texture *texture =
            SDL_CreateTexture(host->renderer, SDL_PIXELFORMAT_ABGR8888, SDL_TEXTUREACCESS_STATIC,
                              image->width, image->height);
        if (!texture) continue;
        SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_BLEND);
        SDL_UpdateTexture(texture, NULL, image->rgba, image->width * 4);
        SDL_Rect src = {image->src_x, image->src_y, image->src_width, image->src_height};
        SDL_Rect dest = {image->dest_x, image->dest_y, image->dest_width, image->dest_height};
        SDL_RenderCopy(host->renderer, texture, &src, &dest);
        SDL_DestroyTexture(texture);
    }

    const AlacCursor *cursor = &info.cursor;
    if (cursor->shape != ALAC_CURSOR_HIDDEN && cursor->row < info.rows) {
        int x = cursor->column * cw, y = cursor->row * ch, width = cursor->wide ? 2 * cw : cw;
        set_color(host->renderer, cursor->color);
        switch (cursor->shape) {
            case ALAC_CURSOR_BLOCK: {
                SDL_RenderFillRect(host->renderer, &(SDL_Rect){x, y, width, ch});
                const AlacCell *cell = &info.cells[cursor->row * info.columns + cursor->column];
                draw_glyph(host, cell, cursor->text_color, x, y);
                break;
            }
            case ALAC_CURSOR_UNDERLINE:
                SDL_RenderFillRect(host->renderer, &(SDL_Rect){x, y + ch - 2, width, 2});
                break;
            case ALAC_CURSOR_BEAM:
                SDL_RenderFillRect(host->renderer, &(SDL_Rect){x, y, 2, ch});
                break;
            default:
                SDL_RenderDrawRect(host->renderer, &(SDL_Rect){x, y, width, ch});
                break;
        }
    }

    SDL_RenderPresent(host->renderer);
}

static void resize(Host *host) {
    int width, height;
    SDL_GetRendererOutputSize(host->renderer, &width, &height);
    uint32_t columns = width / host->cell_width, rows = height / host->cell_height;
    alac_terminal_resize(host->term, columns < 2 ? 2 : columns, rows < 1 ? 1 : rows,
                         host->cell_width, host->cell_height);
}

static uint32_t mods(void) {
    SDL_Keymod state = SDL_GetModState();
    return ((state & KMOD_SHIFT) ? ALAC_MOD_SHIFT : 0) | ((state & KMOD_ALT) ? ALAC_MOD_ALT : 0) |
           ((state & KMOD_CTRL) ? ALAC_MOD_CONTROL : 0) | ((state & KMOD_GUI) ? ALAC_MOD_SUPER : 0);
}

static uint32_t named_key(SDL_Keycode sym) {
    switch (sym) {
        case SDLK_RETURN: return ALAC_KEY_ENTER;
        case SDLK_KP_ENTER: return ALAC_KEY_ENTER;
        case SDLK_TAB: return ALAC_KEY_TAB;
        case SDLK_BACKSPACE: return ALAC_KEY_BACKSPACE;
        case SDLK_ESCAPE: return ALAC_KEY_ESCAPE;
        case SDLK_INSERT: return ALAC_KEY_INSERT;
        case SDLK_DELETE: return ALAC_KEY_DELETE;
        case SDLK_HOME: return ALAC_KEY_HOME;
        case SDLK_END: return ALAC_KEY_END;
        case SDLK_PAGEUP: return ALAC_KEY_PAGE_UP;
        case SDLK_PAGEDOWN: return ALAC_KEY_PAGE_DOWN;
        case SDLK_UP: return ALAC_KEY_UP;
        case SDLK_DOWN: return ALAC_KEY_DOWN;
        case SDLK_LEFT: return ALAC_KEY_LEFT;
        case SDLK_RIGHT: return ALAC_KEY_RIGHT;
        default:
            if (sym >= SDLK_F1 && sym <= SDLK_F12) return ALAC_KEY_F1 + (sym - SDLK_F1);
            return 0;
    }
}

static void key_down(Host *host, const SDL_KeyboardEvent *event) {
    SDL_Keymod state = SDL_GetModState();
    SDL_Keycode sym = event->keysym.sym;
    if ((state & COPY_MODS) == COPY_MODS && (sym == SDLK_c || sym == SDLK_v)) {
        if (sym == SDLK_c) alac_terminal_copy_selection(host->term, ALAC_CLIPBOARD);
        else alac_terminal_paste_clipboard(host->term, ALAC_CLIPBOARD);
        return;
    }

    AlacKeyEvent key = {
        .action = event->repeat ? ALAC_KEY_REPEAT : ALAC_KEY_PRESS,
        .mods = mods(),
        .location = sym == SDLK_KP_ENTER ? ALAC_LOCATION_NUMPAD : ALAC_LOCATION_STANDARD,
    };
    key.key = named_key(sym);
    if (key.key == 0) {
        /* Printable keys arrive as SDL_TEXTINPUT unless Control or Alt is held. */
        if (!(state & (KMOD_CTRL | KMOD_ALT)) || sym < ' ' || sym > '~') return;
        key.codepoint = (state & KMOD_SHIFT) ? SDL_toupper(sym) : sym;
        key.unshifted_codepoint = sym;
    }
    alac_terminal_key(host->term, &key);
}

static void mouse_cell(Host *host, int x, int y, uint32_t *column, uint32_t *row, uint32_t *side) {
    /* Mouse coordinates are in window points; the grid is in output pixels. */
    int window_width, output_width, unused;
    SDL_GetWindowSize(host->window, &window_width, &unused);
    SDL_GetRendererOutputSize(host->renderer, &output_width, &unused);
    float scale = window_width > 0 ? (float)output_width / window_width : 1.f;
    alac_terminal_cell_at(host->term, x * scale, y * scale, column, row, side);
}

static uint32_t mouse_button(uint8_t button) {
    return button == SDL_BUTTON_MIDDLE ? ALAC_MOUSE_MIDDLE
           : button == SDL_BUTTON_RIGHT ? ALAC_MOUSE_RIGHT
                                        : ALAC_MOUSE_LEFT;
}

static void handle(Host *host, const SDL_Event *event) {
    uint32_t column, row, side;
    switch (event->type) {
        case SDL_QUIT:
            host->running = false;
            break;
        case SDL_WINDOWEVENT:
            if (event->window.event == SDL_WINDOWEVENT_SIZE_CHANGED) resize(host);
            if (event->window.event == SDL_WINDOWEVENT_FOCUS_GAINED)
                alac_terminal_set_focused(host->term, true);
            if (event->window.event == SDL_WINDOWEVENT_FOCUS_LOST)
                alac_terminal_set_focused(host->term, false);
            break;
        case SDL_KEYDOWN:
            key_down(host, &event->key);
            break;
        case SDL_TEXTINPUT:
            if (!(SDL_GetModState() & (KMOD_CTRL | KMOD_GUI)))
                alac_terminal_text(host->term, event->text.text, strlen(event->text.text));
            break;
        case SDL_MOUSEBUTTONDOWN:
        case SDL_MOUSEBUTTONUP: {
            bool pressed = event->type == SDL_MOUSEBUTTONDOWN;
            mouse_cell(host, event->button.x, event->button.y, &column, &row, &side);
            if (alac_terminal_mouse_button(host->term, mouse_button(event->button.button), pressed,
                                           column, row, mods()))
                break;
            if (event->button.button != SDL_BUTTON_LEFT) break;
            host->selecting = pressed;
            if (pressed) {
                bool block = SDL_GetModState() & KMOD_CTRL;
                alac_terminal_select_click(host->term, column, row, side, event->button.clicks,
                                           block);
            }
            break;
        }
        case SDL_MOUSEMOTION:
            mouse_cell(host, event->motion.x, event->motion.y, &column, &row, &side);
            if (!alac_terminal_mouse_move(host->term, column, row, mods()) && host->selecting)
                alac_terminal_select_update(host->term, column, row, side);
            break;
        case SDL_MOUSEWHEEL: {
            int x, y;
            SDL_GetMouseState(&x, &y);
            mouse_cell(host, x, y, &column, &row, &side);
            alac_terminal_mouse_wheel(host->term, event->wheel.y * 3, -event->wheel.x, column, row,
                                      mods());
            break;
        }
        default:
            break;
    }
}

static void poll_terminal(Host *host) {
    AlacEvent event;
    while (alac_terminal_poll_event(host->term, &event)) {
        switch (event.kind) {
            case ALAC_EVENT_TITLE:
                SDL_SetWindowTitle(host->window, event.data ? (const char *)event.data : "Terminal");
                break;
            case ALAC_EVENT_CHILD_EXIT:
            case ALAC_EVENT_EXIT:
                host->running = false;
                break;
            default:
                break;
        }
    }
}

int main(int argc, char **argv) {
    const char *font_path = getenv("ALACRITTY_FONT") ? getenv("ALACRITTY_FONT") : DEFAULT_FONT;
    const char *command = NULL, *screenshot = NULL;
    int font_size = 14, exit_after = -1;
    for (int i = 1; i + 1 < argc; i += 2) {
        if (strcmp(argv[i], "--font") == 0) font_path = argv[i + 1];
        else if (strcmp(argv[i], "--size") == 0) font_size = atoi(argv[i + 1]);
        else if (strcmp(argv[i], "--command") == 0) command = argv[i + 1];
        else if (strcmp(argv[i], "--exit-after") == 0) exit_after = atoi(argv[i + 1]);
        else if (strcmp(argv[i], "--screenshot") == 0) screenshot = argv[i + 1];
    }

    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS) != 0 || TTF_Init() != 0) {
        fprintf(stderr, "SDL: %s\n", SDL_GetError());
        return 1;
    }
    static Host host;
    host.running = true;
    host.wakeup_event = SDL_RegisterEvents(1);
    host.window = SDL_CreateWindow("Terminal", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED,
                                   800, 500, SDL_WINDOW_RESIZABLE | SDL_WINDOW_ALLOW_HIGHDPI);
    host.renderer = host.window ? SDL_CreateRenderer(host.window, -1, 0) : NULL;
    if (!host.renderer) {
        fprintf(stderr, "SDL: %s\n", SDL_GetError());
        return 1;
    }

    /* Scale the font for high-DPI outputs. */
    int window_width, output_width, unused;
    SDL_GetWindowSize(host.window, &window_width, &unused);
    SDL_GetRendererOutputSize(host.renderer, &output_width, &unused);
    host.font = TTF_OpenFont(font_path, font_size * output_width / (window_width ? window_width : 1));
    if (!host.font) {
        fprintf(stderr, "font %s: %s\n", font_path, TTF_GetError());
        return 1;
    }
    TTF_GlyphMetrics32(host.font, 'M', NULL, NULL, NULL, NULL, &host.cell_width);
    host.cell_height = TTF_FontLineSkip(host.font);

    AlacOptions options;
    alac_options_default(&options);
    options.cell_width = host.cell_width;
    options.cell_height = host.cell_height;
    AlacPtyOptions pty = {0};
    const char *args[] = {"-c", command};
    if (command) {
        pty.program = "/bin/sh";
        pty.args = args;
        pty.arg_count = 2;
        pty.drain_on_exit = true;
    }
    host.term = alac_terminal_spawn(&options, &pty, wakeup, &host);
    if (!host.term) {
        fprintf(stderr, "spawn: %s\n", alac_last_error());
        return 1;
    }
    AlacClipboardCallbacks clipboard = {NULL, clipboard_formats, clipboard_read, clipboard_write};
    alac_terminal_set_clipboard(host.term, &clipboard);
    host.snapshot = alac_snapshot_new();
    resize(&host);
    SDL_StartTextInput();

    uint32_t start = SDL_GetTicks();
    unsigned frames = 0;
    bool dirty = true;
    while (host.running) {
        if (dirty) {
            draw(&host);
            frames++;
            dirty = false;
        }

        int64_t timeout = alac_terminal_next_timeout_ms(host.term);
        if (exit_after >= 0) {
            int64_t remaining = (int64_t)exit_after - (SDL_GetTicks() - start);
            if (remaining <= 0) break;
            if (timeout < 0 || remaining < timeout) timeout = remaining;
        }

        SDL_Event event;
        if (timeout < 0 ? SDL_WaitEvent(&event) : SDL_WaitEventTimeout(&event, (int)timeout)) {
            do {
                if (event.type != host.wakeup_event) handle(&host, &event);
                dirty = true;
            } while (SDL_PollEvent(&event));
        }
        alac_terminal_tick(host.term);
        poll_terminal(&host);
    }

    if (screenshot) {
        draw(&host);
        int width, height;
        SDL_GetRendererOutputSize(host.renderer, &width, &height);
        SDL_Surface *surface =
            SDL_CreateRGBSurfaceWithFormat(0, width, height, 32, SDL_PIXELFORMAT_ARGB8888);
        SDL_RenderReadPixels(host.renderer, NULL, SDL_PIXELFORMAT_ARGB8888, surface->pixels,
                             surface->pitch);
        SDL_SaveBMP(surface, screenshot);
        SDL_FreeSurface(surface);
    }

    if (command) {
        char *screen = alac_terminal_screen_text(host.term);
        printf("%s\n", screen ? screen : "");
        alac_string_free(screen);
    }
    printf("frames: %u\n", frames);

    alac_snapshot_free(host.snapshot);
    alac_terminal_free(host.term);
    for (int i = 0; i < GLYPH_CACHE; i++)
        if (host.glyphs[i].texture) SDL_DestroyTexture(host.glyphs[i].texture);
    TTF_CloseFont(host.font);
    SDL_DestroyRenderer(host.renderer);
    SDL_DestroyWindow(host.window);
    TTF_Quit();
    SDL_Quit();
    return 0;
}
