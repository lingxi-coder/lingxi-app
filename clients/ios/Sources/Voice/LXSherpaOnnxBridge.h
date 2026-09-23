#ifndef LX_SHERPA_ONNX_BRIDGE_H
#define LX_SHERPA_ONNX_BRIDGE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct LXOnlineRecognizer LXOnlineRecognizer;
typedef struct LXOnlineStream LXOnlineStream;
typedef struct LXAudioCancellationToken LXAudioCancellationToken;

#define LX_SHERPA_TTS_SUCCESS 1
#define LX_SHERPA_TTS_FAILURE 0
#define LX_SHERPA_TTS_MEDIA_TOO_LARGE 2
#define LX_SHERPA_TTS_CANCELLED 3

LXAudioCancellationToken *LXAudioCancellationTokenCreate(void);
void LXAudioCancellationTokenCancel(LXAudioCancellationToken *token);
void LXAudioCancellationTokenDestroy(LXAudioCancellationToken *token);

LXOnlineRecognizer *LXOnlineRecognizerCreate(const char *model_directory);
void LXOnlineRecognizerDestroy(LXOnlineRecognizer *recognizer);
LXOnlineStream *LXOnlineStreamCreate(LXOnlineRecognizer *recognizer);
void LXOnlineStreamAccept(
    LXOnlineRecognizer *recognizer,
    LXOnlineStream *stream,
    const float *samples,
    int32_t sample_count,
    int32_t sample_rate
);
void LXOnlineStreamFinish(LXOnlineRecognizer *recognizer, LXOnlineStream *stream);
char *LXOnlineStreamCopyText(LXOnlineRecognizer *recognizer, LXOnlineStream *stream);
void LXOnlineStreamDestroy(LXOnlineStream *stream);

char *LXOfflineMoonshineCopyText(
    const char *model_directory,
    const float *samples,
    int32_t sample_count,
    int32_t sample_rate
);

int32_t LXSherpaTtsCopyPCM16(
    const char *model_directory,
    const char *model_id,
    const char *text,
    int32_t speaker_id,
    float speed,
    uint64_t maximum_raw_bytes,
    LXAudioCancellationToken *cancellation_token,
    int16_t **samples,
    int32_t *sample_count,
    int32_t *sample_rate
);

int32_t LXExtractTarBz2(const char *archive_path, const char *destination_path);
void LXFree(void *pointer);

#ifdef __cplusplus
}
#endif

#endif
