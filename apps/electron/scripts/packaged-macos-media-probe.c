#include <gst/gst.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int within(const char *directory, const char *path) {
  size_t length = strlen(directory);
  return strncmp(path, directory, length) == 0 &&
         (path[length] == '/' || path[length] == '\0');
}

static int verify_loaded_images(const char *executable, const char *libraries,
                                const char *plugins) {
  for (uint32_t index = 0; index < _dyld_image_count(); index++) {
    const char *path = _dyld_get_image_name(index);
    char resolved[PATH_MAX];
    if (!path) return 0;
    if (within("/usr/lib", path) || within("/System/Library", path)) continue;
    if (!realpath(path, resolved) ||
        (strcmp(resolved, executable) != 0 && !within(libraries, resolved) &&
         !within(plugins, resolved))) {
      fprintf(stderr, "loaded image is outside the packaged runtime: %s\n", path);
      return 0;
    }
  }
  return 1;
}

static int run_pipeline(const char *description) {
  GError *error = NULL;
  GstElement *pipeline = gst_parse_launch(description, &error);
  if (!pipeline || error) {
    fprintf(stderr, "pipeline creation failed: %s\n", error ? error->message : "unknown error");
    if (error) g_error_free(error);
    if (pipeline) gst_object_unref(pipeline);
    return 0;
  }
  GstBus *bus = gst_element_get_bus(pipeline);
  if (!bus || gst_element_set_state(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
    fprintf(stderr, "pipeline failed to start\n");
    if (bus) gst_object_unref(bus);
    gst_object_unref(pipeline);
    return 0;
  }
  GstMessage *message = gst_bus_timed_pop_filtered(
      bus, 15 * GST_SECOND, GST_MESSAGE_EOS | GST_MESSAGE_ERROR);
  int ok = message && GST_MESSAGE_TYPE(message) == GST_MESSAGE_EOS;
  if (!ok) {
    if (message) {
      gchar *detail = NULL;
      gst_message_parse_error(message, &error, &detail);
      fprintf(stderr, "pipeline failed: %s (%s): %s\n", error ? error->message : "unknown error",
              detail ? detail : "no detail", description);
      if (error) g_error_free(error);
      g_free(detail);
    } else {
      fprintf(stderr, "pipeline timed out\n");
    }
  }
  if (message) gst_message_unref(message);
  gst_element_set_state(pipeline, GST_STATE_NULL);
  gst_object_unref(bus);
  gst_object_unref(pipeline);
  return ok;
}

int main(int argc, char **argv) {
  if (argc != 3) return 2;
  char executable[PATH_MAX], plugins[PATH_MAX], libraries[PATH_MAX];
  if (!realpath(argv[0], executable) || !realpath(argv[1], plugins) ||
      !realpath(argv[2], libraries)) return 2;
  char *gst_argv[] = {"lingxi-media", "--gst-disable-registry-update",
                      "--gst-disable-registry-fork", NULL};
  char **gst_args = gst_argv;
  int gst_argc = 3;
  GError *error = NULL;
  if (!gst_init_check(&gst_argc, &gst_args, &error)) {
    fprintf(stderr, "GStreamer initialization failed: %s\n", error ? error->message : "unknown error");
    if (error) g_error_free(error);
    return 3;
  }
  const char *names[] = {"app", "audioconvert", "audioresample", "coreelements",
                         "opus", "rtp", "rtpmanager", "vpx", "videoconvertscale"};
  for (size_t index = 0; index < sizeof(names) / sizeof(names[0]); index++) {
    char path[PATH_MAX], expected[PATH_MAX], actual[PATH_MAX];
    if (snprintf(path, sizeof(path), "%s/libgst%s.dylib", plugins, names[index]) >= (int)sizeof(path) ||
        !realpath(path, expected) || !within(plugins, expected)) {
      fprintf(stderr, "packaged plugin is missing or escapes its directory: %s\n", names[index]);
      return 4;
    }
    GstPlugin *plugin = gst_plugin_load_file(expected, &error);
    if (!plugin) {
      fprintf(stderr, "plugin %s failed to load: %s\n", names[index], error ? error->message : "unknown error");
      if (error) g_error_free(error);
      return 4;
    }
    const char *filename = gst_plugin_get_filename(plugin);
    int ok = filename && realpath(filename, actual) && strcmp(actual, expected) == 0;
    gst_object_unref(plugin);
    if (!ok) {
      fprintf(stderr, "plugin %s was not loaded from its packaged file\n", names[index]);
      return 4;
    }
  }
  const char *factories[] = {"appsrc", "appsink", "audioconvert", "audioresample", "fakesrc", "fakesink",
                             "opusenc", "opusdec", "rtpopuspay", "rtpopusdepay", "rtpjitterbuffer",
                             "vp8enc", "vp8dec", "rtpvp8pay", "rtpvp8depay", "videoconvert", "videoscale"};
  for (size_t index = 0; index < sizeof(factories) / sizeof(factories[0]); index++) {
    GstElementFactory *factory = gst_element_factory_find(factories[index]);
    if (!factory) {
      fprintf(stderr, "required factory is unavailable: %s\n", factories[index]);
      return 5;
    }
    gst_object_unref(factory);
  }
  if (!verify_loaded_images(executable, libraries, plugins)) return 6;
  if (!run_pipeline("fakesrc num-buffers=2 sizetype=fixed sizemax=1920 do-timestamp=true format=time ! "
                    "audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved ! "
                    "audioconvert ! audioresample ! opusenc ! opusdec ! fakesink sync=false") ||
      !run_pipeline("fakesrc num-buffers=2 sizetype=fixed sizemax=16384 filltype=zero format=time ! "
                    "video/x-raw,format=RGBA,width=64,height=64,framerate=30/1 ! "
                    "identity datarate=491520 ! videoconvert ! vp8enc deadline=1 ! vp8dec ! videoconvert ! fakesink sync=false")) return 7;
  if (!verify_loaded_images(executable, libraries, plugins)) return 6;
  puts("packaged media plugins and audio/video pipelines OK");
  return 0;
}
