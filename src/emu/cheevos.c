/* romi's glue to rcheevos (vendor/rcheevos): one rc_client per game played in romi, and the
 * game's memory as rcheevos' libretro helpers lay it out from the core's memory map.
 *
 * Everything rcheevos hands back is copied into plain arguments here, so the Rust side
 * (cheevos.rs) never mirrors one of its structs. Rust gives its callbacks in `romi_rc_callbacks`
 * and a pointer that comes back with each one. */

#include "rc_client.h"
#include "rc_libretro.h"

#include <stdlib.h>
#include <string.h>

/* What happened, for Rust: an unlock, a game loaded, an error. Strings live until the callback
 * returns. */
typedef struct romi_rc_event {
  uint32_t type;
  uint32_t id;
  uint32_t points;
  const char* title;
  const char* description;
  const char* badge_url;
  /* Progress towards it ("3/5"), for progress indicators. */
  const char* progress;
  const char* error;
} romi_rc_event;

typedef struct romi_rc_callbacks {
  /* Make an HTTP request (POST when `post_data` isn't NULL) and answer it, from any thread, with
   * romi_rc_respond(respond, respond_data, ...). Every request must be answered. */
  void (*http)(void* rust, const char* url, const char* post_data, const char* content_type,
               rc_client_server_callback_t respond, void* respond_data);
  void (*event)(void* rust, const romi_rc_event* event);
  /* A login (what = 1) or a game load (what = 2) finished: result is RC_OK or an error. */
  void (*done)(void* rust, int what, int result, const char* error);
  void (*log)(void* rust, const char* message);
} romi_rc_callbacks;

enum { ROMI_RC_LOGIN = 1, ROMI_RC_LOAD = 2 };

/* What a client is given as its userdata. */
typedef struct romi_rc_context {
  romi_rc_callbacks callbacks;
  void* rust;
  rc_libretro_memory_regions_t regions;
} romi_rc_context;

static romi_rc_context* context(const rc_client_t* client) {
  return (romi_rc_context*)rc_client_get_userdata(client);
}

static uint32_t RC_CCONV read_memory(uint32_t address, uint8_t* buffer, uint32_t num_bytes, rc_client_t* client) {
  return rc_libretro_memory_read(&context(client)->regions, address, buffer, num_bytes);
}

static void RC_CCONV server_call(const rc_api_request_t* request, rc_client_server_callback_t callback,
                                 void* callback_data, rc_client_t* client) {
  romi_rc_context* ctx = context(client);
  ctx->callbacks.http(ctx->rust, request->url, request->post_data, request->content_type, callback, callback_data);
}

void romi_rc_respond(rc_client_server_callback_t respond, void* respond_data, const char* body, size_t body_length,
                     int http_status_code) {
  rc_api_server_response_t response;
  response.body = body;
  response.body_length = body_length;
  response.http_status_code = http_status_code;
  respond(&response, respond_data);
}

static void RC_CCONV event_handler(const rc_client_event_t* event, rc_client_t* client) {
  romi_rc_context* ctx = context(client);
  romi_rc_event out;
  char badge_url[512] = "";
  memset(&out, 0, sizeof(out));
  out.type = event->type;
  if (event->achievement) {
    const rc_client_achievement_t* a = event->achievement;
    out.id = a->id;
    out.points = a->points;
    out.title = a->title;
    out.description = a->description;
    out.progress = a->measured_progress;
    if (rc_client_achievement_get_image_url(a, RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED, badge_url, sizeof(badge_url)) == RC_OK)
      out.badge_url = badge_url;
  }
  if (event->server_error) {
    out.error = event->server_error->error_message;
    out.id = event->server_error->related_id;
  }
  if (event->type == RC_CLIENT_EVENT_GAME_COMPLETED) {
    const rc_client_game_t* game = rc_client_get_game_info(client);
    if (game)
      out.title = game->title;
  }
  ctx->callbacks.event(ctx->rust, &out);
}

static void RC_CCONV log_message(const char* message, const rc_client_t* client) {
  romi_rc_context* ctx = context(client);
  ctx->callbacks.log(ctx->rust, message);
}

rc_client_t* romi_rc_create(const romi_rc_callbacks* callbacks, void* rust, const char* host) {
  romi_rc_context* ctx = (romi_rc_context*)calloc(1, sizeof(romi_rc_context));
  rc_client_t* client;
  if (!ctx)
    return NULL;
  client = rc_client_create(read_memory, server_call);
  if (!client) {
    free(ctx);
    return NULL;
  }
  ctx->callbacks = *callbacks;
  ctx->rust = rust;
  rc_client_set_userdata(client, ctx);
  rc_client_set_event_handler(client, event_handler);
  rc_client_enable_logging(client, RC_CLIENT_LOG_LEVEL_WARN, log_message);
  /* Softcore: the server turns hardcore unlocks from clients it doesn't recognize into softcore. */
  rc_client_set_hardcore_enabled(client, 0);
  /* Memory is only read between frames, on the thread running the core, so a reply that comes
   * after the game has stopped never reads memory the core has let go of. */
  rc_client_set_allow_background_memory_reads(client, 0);
  if (host && *host)
    rc_client_set_host(client, host);
  return client;
}

void romi_rc_destroy(rc_client_t* client) {
  romi_rc_context* ctx = context(client);
  rc_client_destroy(client);
  free(ctx);
}

/* The client whose memory is being laid out, for rcheevos' account of how it did it. */
static romi_rc_context* laying_out;

static void RC_CCONV memory_message(const char* message) {
  if (laying_out)
    laying_out->callbacks.log(laying_out->rust, message);
}

/* The core's memory functions, for the one client whose game is being read at a time. */
static void* (*current_memory_data)(unsigned id);
static size_t (*current_memory_size)(unsigned id);

static void RC_CCONV core_memory_info(uint32_t id, rc_libretro_core_memory_info_t* info) {
  info->data = current_memory_data ? (uint8_t*)current_memory_data(id) : NULL;
  info->size = current_memory_size ? current_memory_size(id) : 0;
  if (!info->data)
    info->size = 0;
}

/* Lays out the game's memory as RetroAchievements numbers it for `console_id`, from the core's
 * memory map if it gave one (NULL if not) and its memory otherwise. Returns non-zero if any of it
 * could be found. */
int romi_rc_memory_init(rc_client_t* client, const struct retro_memory_map* mmap, uint32_t console_id,
                        void* (*memory_data)(unsigned id), size_t (*memory_size)(unsigned id)) {
  romi_rc_context* ctx = context(client);
  int found;
  current_memory_data = memory_data;
  current_memory_size = memory_size;
  laying_out = ctx;
  rc_libretro_init_verbose_message_callback(memory_message);
  found = rc_libretro_memory_init(&ctx->regions, mmap, core_memory_info, console_id);
  rc_libretro_init_verbose_message_callback(NULL);
  laying_out = NULL;
  current_memory_data = NULL;
  current_memory_size = NULL;
  return found;
}

static void RC_CCONV finished(int what, int result, const char* error_message, rc_client_t* client) {
  romi_rc_context* ctx = context(client);
  ctx->callbacks.done(ctx->rust, what, result, error_message);
}

static void RC_CCONV login_done(int result, const char* error_message, rc_client_t* client, void* userdata) {
  (void)userdata;
  finished(ROMI_RC_LOGIN, result, error_message, client);
}

static void RC_CCONV load_done(int result, const char* error_message, rc_client_t* client, void* userdata) {
  (void)userdata;
  finished(ROMI_RC_LOAD, result, error_message, client);
}

void romi_rc_login_with_password(rc_client_t* client, const char* username, const char* password) {
  rc_client_begin_login_with_password(client, username, password, login_done, NULL);
}

void romi_rc_login_with_token(rc_client_t* client, const char* username, const char* token) {
  rc_client_begin_login_with_token(client, username, token, login_done, NULL);
}

void romi_rc_load_game(rc_client_t* client, const char* hash) {
  rc_client_begin_load_game(client, hash, load_done, NULL);
}

/* The logged in user's name and login token, or NULLs before login. */
void romi_rc_user(const rc_client_t* client, const char** username, const char** token) {
  const rc_client_user_t* user = rc_client_get_user_info(client);
  *username = user ? user->username : NULL;
  *token = user ? user->token : NULL;
}

/* The loaded game's title, and how many of its achievements and points are unlocked. */
const char* romi_rc_game(rc_client_t* client, uint32_t* unlocked, uint32_t* total, uint32_t* points_unlocked,
                         uint32_t* points_total) {
  const rc_client_game_t* game = rc_client_get_game_info(client);
  rc_client_user_game_summary_t summary;
  memset(&summary, 0, sizeof(summary));
  rc_client_get_user_game_summary(client, &summary);
  *unlocked = summary.num_unlocked_achievements;
  *total = summary.num_core_achievements;
  *points_unlocked = summary.points_unlocked;
  *points_total = summary.points_core;
  return game ? game->title : NULL;
}

size_t romi_rc_user_agent_clause(rc_client_t* client, char* buffer, size_t size) {
  return rc_client_get_user_agent_clause(client, buffer, size);
}
