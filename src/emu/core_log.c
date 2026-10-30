/* What a libretro core logs, for romi. Cores log through a printf-like function the frontend gives
 * them, and Rust can't define a function that takes `...`, so this one formats the message and
 * hands it to the function Rust gave in `romi_core_log_to`. */

#include "libretro.h"

#include <stdarg.h>
#include <stdio.h>

static void (*sink)(int level, const char* message);

static void log_printf(enum retro_log_level level, const char* fmt, ...) {
  char message[2048];
  va_list args;
  if (!sink) {
    return;
  }
  va_start(args, fmt);
  vsnprintf(message, sizeof message, fmt, args);
  va_end(args);
  sink((int)level, message);
}

/* Sends what cores log to `to`, and gives back the function to hand cores for it. */
retro_log_printf_t romi_core_log_to(void (*to)(int level, const char* message)) {
  sink = to;
  return log_printf;
}
