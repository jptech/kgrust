// A pseudo-terminal for a simulated core's serial pins (the `serve`
// testbench, crates/kg-board/src/bench.rs, or a board's own bridge):
// programs on the host open its slave, /dev/ttys* or /dev/pts/*, as they
// would the board's FT232H port, and the testbench moves bytes between the
// master and the core. Called through Verilator's DPI-C.

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <termios.h>
#include <unistd.h>

namespace {
int master = -1;
}

// The front panel's, when vboard.cpp is linked beside this (the generated
// `serve` testbench), so the page can name the port; weak, so a board's own
// bridge testbench links without it.
extern "C" void vb_set_pty(const char* path) __attribute__((weak));

// Open the pseudo-terminal, write its slave's path to `path_file` (for the
// script that waits on it) and to stdout. Returns 0, or -1 on failure.
extern "C" int uart_pty_open(const char* path_file) {
    master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0 || grantpt(master) != 0 || unlockpt(master) != 0) {
        perror("uart_pty: posix_openpt");
        return -1;
    }
    const char* slave = ptsname(master);
    if (slave == nullptr) {
        perror("uart_pty: ptsname");
        return -1;
    }
    // Raw, so bytes pass both ways untouched until a client sets its own mode.
    int fd = open(slave, O_RDWR | O_NOCTTY);
    if (fd >= 0) {
        struct termios t;
        if (tcgetattr(fd, &t) == 0) {
            cfmakeraw(&t);
            tcsetattr(fd, TCSANOW, &t);
        }
        close(fd);
    }
    fcntl(master, F_SETFL, fcntl(master, F_GETFL) | O_NONBLOCK);
    if (FILE* f = fopen(path_file, "w")) {
        fprintf(f, "%s\n", slave);
        fclose(f);
    }
    if (vb_set_pty) vb_set_pty(slave);
    printf("uart_pty: %s\n", slave);
    fflush(stdout);
    return 0;
}

// The next byte a client wrote, or -1 if there is none yet.
extern "C" int uart_pty_getc() {
    unsigned char c;
    ssize_t n = read(master, &c, 1);
    return n == 1 ? c : -1;
}

// A byte for the client. Dropped if no client has the slave open, or its
// input buffer is full, as a real port without flow control drops it.
extern "C" void uart_pty_putc(int c) {
    unsigned char b = static_cast<unsigned char>(c);
    ssize_t n = write(master, &b, 1);
    (void)n;
}
