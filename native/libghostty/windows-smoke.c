#define GHOSTTY_STATIC
#include <ghostty/vt.h>

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

int main(void) {
    bool simd = false;
    GhosttyOptimizeMode optimize = GHOSTTY_OPTIMIZE_DEBUG;
    if (ghostty_build_info(GHOSTTY_BUILD_INFO_SIMD, &simd) != GHOSTTY_SUCCESS || !simd) {
        fputs("libghostty smoke requires SIMD\n", stderr);
        return 1;
    }
    if (ghostty_build_info(GHOSTTY_BUILD_INFO_OPTIMIZE, &optimize) != GHOSTTY_SUCCESS ||
        optimize != GHOSTTY_OPTIMIZE_RELEASE_FAST) {
        fputs("libghostty smoke requires ReleaseFast\n", stderr);
        return 1;
    }

    GhosttyTerminal terminal = NULL;
    if (ghostty_terminal_new(NULL, &terminal, 80, 24) != GHOSTTY_SUCCESS || terminal == NULL) {
        fputs("ghostty_terminal_new failed\n", stderr);
        return 1;
    }

    static const uint8_t fixture[] = "PANEFLOW\x1b[31m_GHOSTTY_MSVC_OK\x1b[0m";
    ghostty_terminal_vt_write(terminal, fixture, sizeof(fixture) - 1);
    ghostty_terminal_free(terminal);
    return 0;
}
