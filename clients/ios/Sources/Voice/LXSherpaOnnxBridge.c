#include "LXSherpaOnnxBridge.h"

#include <bzlib.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <math.h>
#include <sherpa-onnx/c-api/c-api.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

struct LXOnlineRecognizer {
    const SherpaOnnxOnlineRecognizer *value;
};

struct LXOnlineStream {
    const SherpaOnnxOnlineStream *value;
};

static void LXPath(char *output, size_t capacity, const char *directory, const char *name) {
    snprintf(output, capacity, "%s/%s", directory, name);
}

LXOnlineRecognizer *LXOnlineRecognizerCreate(const char *model_directory) {
    if (model_directory == NULL) return NULL;
    char encoder[PATH_MAX], decoder[PATH_MAX], joiner[PATH_MAX], tokens[PATH_MAX];
    LXPath(encoder, sizeof(encoder), model_directory, "encoder-epoch-99-avg-1.int8.onnx");
    LXPath(decoder, sizeof(decoder), model_directory, "decoder-epoch-99-avg-1.onnx");
    LXPath(joiner, sizeof(joiner), model_directory, "joiner-epoch-99-avg-1.int8.onnx");
    LXPath(tokens, sizeof(tokens), model_directory, "tokens.txt");

    SherpaOnnxOnlineRecognizerConfig config;
    memset(&config, 0, sizeof(config));
    config.feat_config.sample_rate = 16000;
    config.feat_config.feature_dim = 80;
    config.model_config.transducer.encoder = encoder;
    config.model_config.transducer.decoder = decoder;
    config.model_config.transducer.joiner = joiner;
    config.model_config.tokens = tokens;
    config.model_config.num_threads = 2;
    config.model_config.provider = "cpu";
    config.decoding_method = "greedy_search";

    const SherpaOnnxOnlineRecognizer *value = SherpaOnnxCreateOnlineRecognizer(&config);
    if (value == NULL) return NULL;
    LXOnlineRecognizer *recognizer = calloc(1, sizeof(*recognizer));
    if (recognizer == NULL) {
        SherpaOnnxDestroyOnlineRecognizer(value);
        return NULL;
    }
    recognizer->value = value;
    return recognizer;
}

void LXOnlineRecognizerDestroy(LXOnlineRecognizer *recognizer) {
    if (recognizer == NULL) return;
    if (recognizer->value != NULL) SherpaOnnxDestroyOnlineRecognizer(recognizer->value);
    free(recognizer);
}

LXOnlineStream *LXOnlineStreamCreate(LXOnlineRecognizer *recognizer) {
    if (recognizer == NULL || recognizer->value == NULL) return NULL;
    const SherpaOnnxOnlineStream *value = SherpaOnnxCreateOnlineStream(recognizer->value);
    if (value == NULL) return NULL;
    LXOnlineStream *stream = calloc(1, sizeof(*stream));
    if (stream == NULL) {
        SherpaOnnxDestroyOnlineStream(value);
        return NULL;
    }
    stream->value = value;
    return stream;
}

void LXOnlineStreamAccept(
    LXOnlineRecognizer *recognizer,
    LXOnlineStream *stream,
    const float *samples,
    int32_t sample_count,
    int32_t sample_rate
) {
    if (recognizer == NULL || stream == NULL || samples == NULL || sample_count <= 0) return;
    SherpaOnnxOnlineStreamAcceptWaveform(stream->value, sample_rate, samples, sample_count);
    while (SherpaOnnxIsOnlineStreamReady(recognizer->value, stream->value)) {
        SherpaOnnxDecodeOnlineStream(recognizer->value, stream->value);
    }
}

void LXOnlineStreamFinish(LXOnlineRecognizer *recognizer, LXOnlineStream *stream) {
    if (recognizer == NULL || stream == NULL) return;
    SherpaOnnxOnlineStreamInputFinished(stream->value);
    while (SherpaOnnxIsOnlineStreamReady(recognizer->value, stream->value)) {
        SherpaOnnxDecodeOnlineStream(recognizer->value, stream->value);
    }
}

char *LXOnlineStreamCopyText(LXOnlineRecognizer *recognizer, LXOnlineStream *stream) {
    if (recognizer == NULL || stream == NULL) return NULL;
    const SherpaOnnxOnlineRecognizerResult *result =
        SherpaOnnxGetOnlineStreamResult(recognizer->value, stream->value);
    if (result == NULL) return NULL;
    char *text = result->text == NULL ? strdup("") : strdup(result->text);
    SherpaOnnxDestroyOnlineRecognizerResult(result);
    return text;
}

void LXOnlineStreamDestroy(LXOnlineStream *stream) {
    if (stream == NULL) return;
    if (stream->value != NULL) SherpaOnnxDestroyOnlineStream(stream->value);
    free(stream);
}

char *LXOfflineMoonshineCopyText(
    const char *model_directory,
    const float *samples,
    int32_t sample_count,
    int32_t sample_rate
) {
    if (model_directory == NULL || samples == NULL || sample_count <= 0) return NULL;
    char preprocessor[PATH_MAX], encoder[PATH_MAX], uncached[PATH_MAX], cached[PATH_MAX], tokens[PATH_MAX];
    LXPath(preprocessor, sizeof(preprocessor), model_directory, "preprocess.onnx");
    LXPath(encoder, sizeof(encoder), model_directory, "encode.int8.onnx");
    LXPath(uncached, sizeof(uncached), model_directory, "uncached_decode.int8.onnx");
    LXPath(cached, sizeof(cached), model_directory, "cached_decode.int8.onnx");
    LXPath(tokens, sizeof(tokens), model_directory, "tokens.txt");

    SherpaOnnxOfflineRecognizerConfig config;
    memset(&config, 0, sizeof(config));
    config.feat_config.sample_rate = 16000;
    config.feat_config.feature_dim = 80;
    config.model_config.moonshine.preprocessor = preprocessor;
    config.model_config.moonshine.encoder = encoder;
    config.model_config.moonshine.uncached_decoder = uncached;
    config.model_config.moonshine.cached_decoder = cached;
    config.model_config.tokens = tokens;
    config.model_config.num_threads = 2;
    config.model_config.provider = "cpu";
    config.decoding_method = "greedy_search";

    const SherpaOnnxOfflineRecognizer *recognizer = SherpaOnnxCreateOfflineRecognizer(&config);
    if (recognizer == NULL) return NULL;
    const SherpaOnnxOfflineStream *stream = SherpaOnnxCreateOfflineStream(recognizer);
    if (stream == NULL) {
        SherpaOnnxDestroyOfflineRecognizer(recognizer);
        return NULL;
    }
    SherpaOnnxAcceptWaveformOffline(stream, sample_rate, samples, sample_count);
    SherpaOnnxDecodeOfflineStream(recognizer, stream);
    const SherpaOnnxOfflineRecognizerResult *result = SherpaOnnxGetOfflineStreamResult(stream);
    char *text = result == NULL || result->text == NULL ? strdup("") : strdup(result->text);
    if (result != NULL) SherpaOnnxDestroyOfflineRecognizerResult(result);
    SherpaOnnxDestroyOfflineStream(stream);
    SherpaOnnxDestroyOfflineRecognizer(recognizer);
    return text;
}

int32_t LXSherpaTtsCopyPCM16(
    const char *model_directory,
    const char *model_id,
    const char *text,
    int32_t speaker_id,
    float speed,
    int16_t **samples,
    int32_t *sample_count,
    int32_t *sample_rate
) {
    if (model_directory == NULL || model_id == NULL || text == NULL ||
        samples == NULL || sample_count == NULL || sample_rate == NULL) return 0;
    *samples = NULL;
    *sample_count = 0;
    *sample_rate = 0;

    SherpaOnnxOfflineTtsConfig config;
    memset(&config, 0, sizeof(config));
    config.model.num_threads = 2;
    config.model.provider = "cpu";
    config.max_num_sentences = 2;

    char model[PATH_MAX], voices[PATH_MAX], tokens[PATH_MAX], lexicon[PATH_MAX];
    char data_dir[PATH_MAX], dict_dir[PATH_MAX], rule_fsts[PATH_MAX * 3];
    LXPath(tokens, sizeof(tokens), model_directory, "tokens.txt");
    if (strstr(model_id, "kitten") != NULL) {
        LXPath(model, sizeof(model), model_directory, "model.fp16.onnx");
        LXPath(voices, sizeof(voices), model_directory, "voices.bin");
        LXPath(data_dir, sizeof(data_dir), model_directory, "espeak-ng-data");
        config.model.kitten.model = model;
        config.model.kitten.voices = voices;
        config.model.kitten.tokens = tokens;
        config.model.kitten.data_dir = data_dir;
        config.model.kitten.length_scale = 1.0f;
    } else {
        LXPath(model, sizeof(model), model_directory, "model.int8.onnx");
        LXPath(lexicon, sizeof(lexicon), model_directory, "lexicon.txt");
        LXPath(dict_dir, sizeof(dict_dir), model_directory, "dict");
        config.model.vits.model = model;
        config.model.vits.lexicon = lexicon;
        config.model.vits.tokens = tokens;
        config.model.vits.dict_dir = dict_dir;
        config.model.vits.noise_scale = 0.667f;
        config.model.vits.noise_scale_w = 0.8f;
        config.model.vits.length_scale = 1.0f;
        char phone[PATH_MAX], date[PATH_MAX], number[PATH_MAX];
        LXPath(phone, sizeof(phone), model_directory, "phone.fst");
        LXPath(date, sizeof(date), model_directory, "date.fst");
        LXPath(number, sizeof(number), model_directory, "number.fst");
        snprintf(rule_fsts, sizeof(rule_fsts), "%s,%s,%s", phone, date, number);
        config.rule_fsts = rule_fsts;
    }

    const SherpaOnnxOfflineTts *tts = SherpaOnnxCreateOfflineTts(&config);
    if (tts == NULL) return 0;
    SherpaOnnxGenerationConfig generation;
    memset(&generation, 0, sizeof(generation));
    generation.sid = speaker_id;
    generation.speed = fminf(2.0f, fmaxf(0.5f, speed));
    generation.silence_scale = 0.2f;
    const SherpaOnnxGeneratedAudio *audio =
        SherpaOnnxOfflineTtsGenerateWithConfig(tts, text, &generation, NULL, NULL);
    if (audio == NULL || audio->samples == NULL || audio->n <= 0) {
        if (audio != NULL) SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
        SherpaOnnxDestroyOfflineTts(tts);
        return 0;
    }
    int16_t *pcm = malloc((size_t)audio->n * sizeof(int16_t));
    if (pcm == NULL) {
        SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
        SherpaOnnxDestroyOfflineTts(tts);
        return 0;
    }
    for (int32_t i = 0; i < audio->n; ++i) {
        float value = fminf(1.0f, fmaxf(-1.0f, audio->samples[i]));
        pcm[i] = (int16_t)lrintf(value * 32767.0f);
    }
    *samples = pcm;
    *sample_count = audio->n;
    *sample_rate = audio->sample_rate;
    SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
    SherpaOnnxDestroyOfflineTts(tts);
    return 1;
}

static int LXSafeArchivePath(const char *path) {
    if (path == NULL || path[0] == '\0' || path[0] == '/') return 0;
    const char *segment = path;
    while (*segment != '\0') {
        while (*segment == '/') segment++;
        const char *end = strchr(segment, '/');
        size_t length = end == NULL ? strlen(segment) : (size_t)(end - segment);
        if (length == 2 && segment[0] == '.' && segment[1] == '.') return 0;
        if (end == NULL) break;
        segment = end + 1;
    }
    return 1;
}

static int LXMakeDirectories(const char *path) {
    char copy[PATH_MAX];
    if (strlen(path) >= sizeof(copy)) return 0;
    strcpy(copy, path);
    for (char *p = copy + 1; *p != '\0'; ++p) {
        if (*p != '/') continue;
        *p = '\0';
        if (mkdir(copy, 0755) != 0 && errno != EEXIST) return 0;
        *p = '/';
    }
    if (mkdir(copy, 0755) != 0 && errno != EEXIST) return 0;
    return 1;
}

static uint64_t LXTarSize(const unsigned char *field, size_t length) {
    uint64_t result = 0;
    for (size_t i = 0; i < length && field[i] != '\0' && field[i] != ' '; ++i) {
        if (field[i] < '0' || field[i] > '7') continue;
        result = result * 8 + (uint64_t)(field[i] - '0');
    }
    return result;
}

static int LXWriteAll(int fd, const unsigned char *bytes, size_t count) {
    size_t offset = 0;
    while (offset < count) {
        ssize_t written = write(fd, bytes + offset, count - offset);
        if (written <= 0) return 0;
        offset += (size_t)written;
    }
    return 1;
}

int32_t LXExtractTarBz2(const char *archive_path, const char *destination_path) {
    if (archive_path == NULL || destination_path == NULL) return 0;
    if (!LXMakeDirectories(destination_path)) return 0;
    char tar_template[PATH_MAX];
    snprintf(tar_template, sizeof(tar_template), "%s/.voice-model.XXXXXX", destination_path);
    int tar_fd = mkstemp(tar_template);
    if (tar_fd < 0) return 0;

    BZFILE *input = BZ2_bzopen(archive_path, "rb");
    if (input == NULL) {
        close(tar_fd);
        unlink(tar_template);
        return 0;
    }
    unsigned char buffer[64 * 1024];
    int read_count;
    int ok = 1;
    while ((read_count = BZ2_bzread(input, buffer, sizeof(buffer))) > 0) {
        ssize_t offset = 0;
        while (offset < read_count) {
            ssize_t written = write(tar_fd, buffer + offset, (size_t)(read_count - offset));
            if (written <= 0) { ok = 0; break; }
            offset += written;
        }
        if (!ok) break;
    }
    BZ2_bzclose(input);
    if (read_count < 0) ok = 0;
    if (lseek(tar_fd, 0, SEEK_SET) < 0) ok = 0;

    unsigned char header[512];
    char pending_long_name[PATH_MAX] = "";
    while (ok && read(tar_fd, header, sizeof(header)) == sizeof(header)) {
        int empty = 1;
        for (size_t i = 0; i < sizeof(header); ++i) if (header[i] != 0) { empty = 0; break; }
        if (empty) break;
        char name[PATH_MAX];
        if (pending_long_name[0] != '\0') {
            strcpy(name, pending_long_name);
            pending_long_name[0] = '\0';
        } else {
            size_t name_length = strnlen((const char *)header, 100);
            size_t prefix_length = strnlen((const char *)header + 345, 155);
            if (prefix_length > 0) {
                snprintf(name, sizeof(name), "%.*s/%.*s",
                         (int)prefix_length, header + 345,
                         (int)name_length, header);
            } else {
                memcpy(name, header, name_length);
                name[name_length] = '\0';
            }
        }
        if (!LXSafeArchivePath(name)) { ok = 0; break; }
        uint64_t size = LXTarSize(header + 124, 12);
        char type = (char)header[156];
        if (type == 'L') {
            size_t copied = size < sizeof(pending_long_name) - 1
                ? (size_t)size : sizeof(pending_long_name) - 1;
            if (read(tar_fd, pending_long_name, copied) != (ssize_t)copied) { ok = 0; break; }
            pending_long_name[copied] = '\0';
            if (size > copied && lseek(tar_fd, (off_t)(size - copied), SEEK_CUR) < 0) { ok = 0; break; }
            uint64_t padding = (512 - (size % 512)) % 512;
            if (padding > 0 && lseek(tar_fd, (off_t)padding, SEEK_CUR) < 0) ok = 0;
            continue;
        }
        char output[PATH_MAX];
        if (snprintf(output, sizeof(output), "%s/%s", destination_path, name) >= (int)sizeof(output)) {
            ok = 0;
            break;
        }
        if (type == '5') {
            ok = LXMakeDirectories(output);
        } else if (type == '0' || type == '\0') {
            char parent[PATH_MAX];
            strcpy(parent, output);
            char *slash = strrchr(parent, '/');
            if (slash != NULL) { *slash = '\0'; ok = LXMakeDirectories(parent); }
            int output_fd = ok ? open(output, O_WRONLY | O_CREAT | O_TRUNC, 0644) : -1;
            if (output_fd < 0) ok = 0;
            uint64_t remaining = size;
            while (ok && remaining > 0) {
                size_t chunk = remaining < sizeof(buffer) ? (size_t)remaining : sizeof(buffer);
                ssize_t count = read(tar_fd, buffer, chunk);
                if (count <= 0 || !LXWriteAll(output_fd, buffer, (size_t)count)) { ok = 0; break; }
                remaining -= (uint64_t)count;
            }
            if (output_fd >= 0) close(output_fd);
        } else {
            if (lseek(tar_fd, (off_t)size, SEEK_CUR) < 0) ok = 0;
        }
        uint64_t padding = (512 - (size % 512)) % 512;
        if (ok && padding > 0 && lseek(tar_fd, (off_t)padding, SEEK_CUR) < 0) ok = 0;
    }
    close(tar_fd);
    unlink(tar_template);
    return ok ? 1 : 0;
}

void LXFree(void *pointer) {
    free(pointer);
}
