#include <alsa/asoundlib.h>
#include <stdarg.h>
#include <stdio.h>

static void (*seneschal_alsa_message_sink)(const char *msg) = NULL;

void seneschal_alsa_set_message_sink(void (*sink)(const char *msg)) {
    seneschal_alsa_message_sink = sink;
}

static void seneschal_alsa_log_handler(const char *file, int line,
                                       const char *func, int err,
                                       const char *fmt, ...) {
    (void)file;
    (void)line;
    (void)err;
    if (seneschal_alsa_message_sink == NULL) {
        return;
    }
    char buf[1024];
    size_t off = 0;
    if (func != NULL && func[0] != '\0') {
        int n = snprintf(buf, sizeof(buf), "%s: ", func);
        if (n > 0 && (size_t)n < sizeof(buf)) {
            off = (size_t)n;
        }
    }
    va_list args;
    va_start(args, fmt);
    vsnprintf(buf + off, sizeof(buf) - off, fmt, args);
    va_end(args);
    seneschal_alsa_message_sink(buf);
}

void seneschal_alsa_silence(void) {
    snd_lib_error_set_handler(seneschal_alsa_log_handler);
}