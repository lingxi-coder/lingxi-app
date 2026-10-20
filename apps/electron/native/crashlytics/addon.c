#include <node_api.h>
#include <stdint.h>

extern int32_t lingxi_crashlytics_start(const char *, const char *);
extern void lingxi_crashlytics_event(const char *, int32_t);
extern int32_t lingxi_crashlytics_previous_crash(void);

static napi_value start(napi_env env, napi_callback_info info) {
  size_t count = 2, length;
  napi_value args[2], result;
  char path[4096], channel[32];
  if (napi_get_cb_info(env, info, &count, args, NULL, NULL) != napi_ok || count != 2 ||
      napi_get_value_string_utf8(env, args[0], NULL, 0, &length) != napi_ok || length >= sizeof(path) ||
      napi_get_value_string_utf8(env, args[0], path, sizeof(path), &length) != napi_ok ||
      napi_get_value_string_utf8(env, args[1], NULL, 0, &length) != napi_ok || length >= sizeof(channel) ||
      napi_get_value_string_utf8(env, args[1], channel, sizeof(channel), &length) != napi_ok) {
    napi_throw_type_error(env, NULL, "Invalid Crashlytics configuration"); return NULL;
  }
  napi_get_boolean(env, lingxi_crashlytics_start(path, channel) == 1, &result);
  return result;
}

static napi_value event(napi_env env, napi_callback_info info) {
  size_t count = 2, length;
  napi_value args[2], result;
  char category[64]; int32_t code;
  if (napi_get_cb_info(env, info, &count, args, NULL, NULL) != napi_ok || count != 2 ||
      napi_get_value_string_utf8(env, args[0], NULL, 0, &length) != napi_ok || length >= sizeof(category) ||
      napi_get_value_string_utf8(env, args[0], category, sizeof(category), &length) != napi_ok ||
      napi_get_value_int32(env, args[1], &code) != napi_ok) {
    napi_throw_type_error(env, NULL, "Invalid Crashlytics event"); return NULL;
  }
  lingxi_crashlytics_event(category, code);
  napi_get_undefined(env, &result); return result;
}

static napi_value previous_crash(napi_env env, napi_callback_info info) {
  napi_value result;
  napi_get_boolean(env, lingxi_crashlytics_previous_crash() == 1, &result);
  return result;
}

NAPI_MODULE_INIT() {
  napi_property_descriptor properties[] = {
    {"start", NULL, start, NULL, NULL, NULL, napi_default, NULL},
    {"recordEvent", NULL, event, NULL, NULL, NULL, napi_default, NULL},
    {"previousExecutionCrashed", NULL, previous_crash, NULL, NULL, NULL, napi_default, NULL}
  };
  napi_define_properties(env, exports, 3, properties);
  return exports;
}
