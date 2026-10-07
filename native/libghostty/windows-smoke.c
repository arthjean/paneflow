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

    static const uint8_t multibyte[] = "\x1b[2J\x1b[H\xc3\xa9\xe6\xbc\xa2";
    ghostty_terminal_vt_write(terminal, multibyte, sizeof(multibyte) - 1);
    static const uint16_t columns[] = {0, 1};
    static const uint32_t expected[] = {0x00E9, 0x6F22};
    for (size_t index = 0; index < sizeof(expected) / sizeof(expected[0]); index++) {
        GhosttyPoint point = {.tag = GHOSTTY_POINT_TAG_ACTIVE};
        point.value.coordinate.x = columns[index];
        point.value.coordinate.y = 0;
        GhosttyGridRef ref = {.size = sizeof(GhosttyGridRef)};
        uint32_t codepoints[4] = {0};
        size_t count = 0;
        if (ghostty_terminal_grid_ref(terminal, point, &ref) != GHOSTTY_SUCCESS ||
            ghostty_grid_ref_graphemes(&ref, codepoints, 4, &count) != GHOSTTY_SUCCESS ||
            count != 1 || codepoints[0] != expected[index]) {
            fprintf(stderr, "UTF-8 smoke expected U+%04X at column %u\n",
                    (unsigned)expected[index], (unsigned)columns[index]);
            ghostty_terminal_free(terminal);
            return 1;
        }
    }

    ghostty_terminal_free(terminal);
    return 0;
}
