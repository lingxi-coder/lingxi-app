package com.lingxi.code.voice.audio

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/**
 * Android system `SpeechRecognizer` wrapped as [SttProvider].
 *
 * Constraints:
 * - System STT consumes the live mic, not pre-recorded files; calls with
 *   [AudioInput.EncodedFile] return `invalid_audio`.
 * - Even the [AudioInput.Pcm16] path is best-effort here — Android system
 *   STT does not accept raw PCM frames as input. This wrapper uses
 *   `RecognizerIntent.EXTRA_LANGUAGE_MODEL` to drive whatever the active
 *   recognition service expects (mic-on); the [audio] payload is ignored,
 *   and the live mic is the actual source.
 *
 * Designed for a talk-mode path where a higher layer opens the mic itself;
 * here the provider just delegates to the system service for transcription.
 *
 * No retry, no streaming partials — `SpeechRecognizer` gives one final
 * transcript per call.
 */
class SystemSpeechRecognizerStt(private val context: Context) : SttProvider {

    override val id: String = "system"

    override val capabilities: SttCapabilities = SttCapabilities(
        streaming = false,
        languages = emptySet(), // system handles per-device default
        maxAudioSeconds = 60,
    )

    override suspend fun transcribe(
        audio: AudioInput,
        language: String?,
        keyProvider: suspend () -> String?,
    ): SttResult {
        if (audio is AudioInput.EncodedFile) {
            return SttResult.Err(
                code = "invalid_audio",
                message = "System STT can't transcribe pre-recorded files; pass a streaming SttProvider.",
                retriable = false,
            )
        }
        if (!SpeechRecognizer.isRecognitionAvailable(context)) {
            return SttResult.Err(
                code = "no_provider_configured",
                message = "Device has no SpeechRecognizer service installed.",
                retriable = false,
            )
        }
        return runOnMainThread { startRecognition(language) }
    }

    private suspend fun startRecognition(language: String?): SttResult =
        suspendCancellableCoroutine { cont ->
            val recognizer = SpeechRecognizer.createSpeechRecognizer(context)
            val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                if (!language.isNullOrBlank()) {
                    putExtra(RecognizerIntent.EXTRA_LANGUAGE, language)
                }
                putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, false)
            }
            recognizer.setRecognitionListener(SimpleListener(recognizer, cont))
            cont.invokeOnCancellation {
                runCatching { recognizer.cancel() }
                runCatching { recognizer.destroy() }
            }
            recognizer.startListening(intent)
        }

    /**
     * `SpeechRecognizer` must be touched from the main thread. The provider
     * may be called from any context, so we hop on demand.
     */
    private suspend fun <T> runOnMainThread(block: suspend () -> T): T {
        return if (Looper.myLooper() == Looper.getMainLooper()) {
            block()
        } else {
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) { block() }
        }
    }

    private class SimpleListener(
        private val recognizer: SpeechRecognizer,
        private val cont: CancellableContinuation<SttResult>,
    ) : RecognitionListener {
        private var resumed = false

        private fun finish(result: SttResult) {
            if (resumed) return
            resumed = true
            runCatching { recognizer.destroy() }
            // Detach via Handler post so we don't recurse back into the listener.
            Handler(Looper.getMainLooper()).post {
                if (cont.isActive) cont.resume(result)
            }
        }

        override fun onResults(results: Bundle?) {
            val candidates = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
            val text = candidates?.firstOrNull().orEmpty()
            finish(SttResult.Ok(text = text, language = null, confidence = null))
        }

        override fun onError(error: Int) {
            val (code, retriable) = when (error) {
                SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "permission_denied" to false
                SpeechRecognizer.ERROR_NETWORK,
                SpeechRecognizer.ERROR_NETWORK_TIMEOUT -> "network_error" to true
                SpeechRecognizer.ERROR_NO_MATCH,
                SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "no_speech" to false
                SpeechRecognizer.ERROR_RECOGNIZER_BUSY -> "audio_io_unavailable" to true
                else -> "provider_error" to false
            }
            finish(SttResult.Err(code = code, message = "SpeechRecognizer error=$error", retriable = retriable))
        }

        // Unused callbacks
        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() {}
        override fun onRmsChanged(rmsdB: Float) {}
        override fun onBufferReceived(buffer: ByteArray?) {}
        override fun onEndOfSpeech() {}
        override fun onPartialResults(partialResults: Bundle?) {}
        override fun onEvent(eventType: Int, params: Bundle?) {}
    }
}
