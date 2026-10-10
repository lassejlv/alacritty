/*
 * Run a command on a PTY, wait for it to exit and print the screen.
 *
 * Usage: pty [command]
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#include "alacritty.h"

int main(int argc, char **argv) {
    const char *command = argc > 1 ? argv[1] : "printf 'hello from \\033[1malacritty\\033[0m'";
    const char *args[] = {"-c", command};

    AlacOptions options;
    alac_options_default(&options);
    AlacPtyOptions pty = {
        .program = "/bin/sh",
        .args = args,
        .arg_count = 2,
        .drain_on_exit = true,
    };

    AlacTerminal *term = alac_terminal_spawn(&options, &pty, NULL, NULL);
    if (!term) {
        fprintf(stderr, "spawn failed: %s\n", alac_last_error());
        return 1;
    }

    /* Real hosts drain events when the wakeup callback fires instead of polling. */
    bool exited = false;
    int exit_code = -1;
    for (int waited = 0; !exited && waited < 10000; waited += 10) {
        AlacEvent event;
        while (alac_terminal_poll_event(term, &event)) {
            if (event.kind == ALAC_EVENT_CHILD_EXIT) exit_code = event.exit_code;
            if (event.kind == ALAC_EVENT_EXIT) exited = true;
        }
        usleep(10000);
    }

    char *screen = alac_terminal_screen_text(term);
    printf("%s\n[exit %d]\n", screen, exit_code);
    alac_string_free(screen);
    alac_terminal_free(term);
    return exited ? exit_code : 1;
}
