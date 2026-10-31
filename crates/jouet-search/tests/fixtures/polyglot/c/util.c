#include <string.h>

struct buffer {
    char *data;
    size_t len;
};

size_t buffer_len(const struct buffer *b) {
    return b->len;
}
