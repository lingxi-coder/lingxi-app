package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.SpeechFfiException
import com.lingxi.code.bindings.TtsAudioFfi
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure result/error-mapping coverage of the UniFFI speech adapters
 * ([AndroidSttAdapter] / [AndroidTtsAdapter]).
 *
 * These adapters only translate the local [SttProvider] / [TtsProvider] success
 * + failure surface onto the generated flat FFI types ([SpeechFfiException],
 * [TtsAudioFfi]) Rust fans back out onto `traits::SttError` / `TtsError`. The
 * translation is what we pin here, driven by in-test fake providers — no engine
 * and no Android framework, so it runs headless on the plain JVM.
 *
 * Constructing the generated UniFFI types is safe off-device: the native `.so`
 * is loaded lazily (`UniffiLib.INSTANCE by lazy`) on the first EXPORTED call,
 * and these data-class / exception constructors are not exported functions.
 *
 * NOT covered (Android-coupled, deliberately skipped — see report):
 *  - [AndroidTtsAdapter]'s `is SystemTextToSpeechTts` fast path: that provider
 *    imports android.speech.tts.TextToSpeech + android.content.Context, so it
 *    can't be instantiated on the JVM. The else branch (any other TtsProvider)
 *    exercises the same TtsAudioFfi assembly + error mapping and IS covered.
 *  - The camera adapter ([com.lingxi.code.vision.AndroidCameraAdapter]): its
 *    only collaborator is the `CameraController` singleton (an `object`, not an
 *    injectable interface) whose capture/pick drive real ActivityResult
 *    launchers, and its `toFfi` mapping is private. There is no pure seam to
 *    exercise without faking the Android UI, so it is skipped rather than faked.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class UniffiSpeechAdaptersTest {

    // --- fakes ----------------------------------------------------------

    private class FakeStt(private val result: SttResult) : SttProvider {
        var lastAudio: AudioInput? = null
        var lastLanguage: String? = null
        override val id = "fake-stt"
        override val capabilities = SttCapabilities(
            streaming = false, languages = emptySet(), maxAudioSeconds = 60,
        )

        override suspend fun transcribe(
            audio: AudioInput,
            language: String?,
            keyProvider: suspend () -> String?,
        ): SttResult {
            lastAudio = audio
            lastLanguage = language
            return result
        }
    }

    private class FakeTts(
        private val chunks: List<ByteArray>,
        private val rate: Int = 24_000,
        private val onSynthesize: (() -> Unit)? = null,
    ) : TtsProvider {
        override val id = "fake-tts"
        override val capabilities = TtsCapabilities(
            streaming = true,
            voices = emptyList(),
            sampleRateHz = rate,
        )

        override suspend fun synthesize(
            text: String,
            voice: String?,
            keyProvider: suspend () -> String?,
        ): Flow<ByteArray> {
            onSynthesize?.invoke()
            return flow { for (c in chunks) emit(c) }
        }
    }

    // --- STT: success ---------------------------------------------------

    @Test
    fun stt_ok_returnsTranscribedText() = runTest {
        val adapter = AndroidSttAdapter(
            FakeStt(SttResult.Ok(text = "hello world", language = "en", confidence = 0.9f)),
        )
        assertEquals("hello world", adapter.transcribe(language = "en-US"))
    }

    @Test
    fun stt_passesLiveMicShapedInput_emptyPcm16At16k() = runTest {
        // The adapter feeds the system recognizer an empty Pcm16 payload (live
        // mic ignores it, but the channel keeps it off the invalid_audio branch).
        val fake = FakeStt(SttResult.Ok("x", null, null))
        AndroidSttAdapter(fake).transcribe(language = null)

        val audio = fake.lastAudio
        assertTrue(audio is AudioInput.Pcm16)
        audio as AudioInput.Pcm16
        assertArrayEquals(ByteArray(0), audio.bytes)
        assertEquals(16_000, audio.sampleRateHz)
        assertEquals(null, fake.lastLanguage)
    }

    // --- STT: error fan-out (the private toSpeechFfiException, via the adapter) ---

    private suspend fun sttErrorFor(code: String, message: String, retriable: Boolean): SpeechFfiException {
        val adapter = AndroidSttAdapter(FakeStt(SttResult.Err(code, message, retriable)))
        return runCatching { adapter.transcribe(language = null) }
            .exceptionOrNull() as SpeechFfiException
    }

    @Test
    fun stt_err_permissionDenied_mapsToPermissionDenied() = runTest {
        val e = sttErrorFor("permission_denied", "mic blocked", retriable = false)
        assertTrue(e is SpeechFfiException.PermissionDenied)
    }

    @Test
    fun stt_err_noSpeech_mapsToNoSpeech() = runTest {
        val e = sttErrorFor("no_speech", "silence", retriable = false)
        assertTrue(e is SpeechFfiException.NoSpeech)
    }

    @Test
    fun stt_err_noProviderConfigured_mapsToUnavailable() = runTest {
        val e = sttErrorFor("no_provider_configured", "none", retriable = false)
        assertTrue(e is SpeechFfiException.Unavailable)
    }

    @Test
    fun stt_err_retriableUnknownCode_mapsToRetriable_carryingMessage() = runTest {
        val e = sttErrorFor("network_blip", "try again", retriable = true)
        assertTrue(e is SpeechFfiException.Retriable)
        assertEquals("try again", (e as SpeechFfiException.Retriable).message)
    }

    @Test
    fun stt_err_unknownNonRetriable_fallsBackToOther_carryingMessage() = runTest {
        val e = sttErrorFor("weird_failure", "boom", retriable = false)
        assertTrue(e is SpeechFfiException.Other)
        assertEquals("boom", (e as SpeechFfiException.Other).message)
    }

    @Test
    fun stt_err_codeWins_overRetriableFlag() = runTest {
        // A recognized code is matched before the retriable fallback, even when
        // the err is flagged retriable.
        val e = sttErrorFor("permission_denied", "blocked", retriable = true)
        assertTrue(e is SpeechFfiException.PermissionDenied)
    }

    // --- TTS: else branch (generic TtsProvider) -------------------------

    @Test
    fun tts_concatenatesFlowChunks_inOrder_intoTtsAudioFfi() = runTest {
        val chunks = listOf(
            byteArrayOf(1, 2, 3),
            byteArrayOf(4, 5),
            byteArrayOf(6, 7, 8, 9),
        )
        val adapter = AndroidTtsAdapter(FakeTts(chunks, rate = 24_000))

        val out: TtsAudioFfi = adapter.synthesize(text = "hi", voice = null)
        assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8, 9), out.pcm)
        assertEquals(24_000u, out.sampleRateHz)
    }

    @Test
    fun tts_reportsProviderSampleRate() = runTest {
        val adapter = AndroidTtsAdapter(FakeTts(listOf(byteArrayOf(0)), rate = 16_000))
        val out = adapter.synthesize(text = "x", voice = "v1")
        assertEquals(16_000u, out.sampleRateHz)
    }

    @Test
    fun tts_emptyStream_yieldsEmptyPcm() = runTest {
        val adapter = AndroidTtsAdapter(FakeTts(emptyList(), rate = 22_050))
        val out = adapter.synthesize(text = "", voice = null)
        assertEquals(0, out.pcm.size)
        assertEquals(22_050u, out.sampleRateHz)
    }

    @Test
    fun tts_passesThroughSpeechFfiException_unwrapped() = runTest {
        // A SpeechFfiException raised by the provider must propagate as-is, not
        // get re-wrapped in Other.
        val failing = object : TtsProvider {
            override val id = "boom"
            override val capabilities = TtsCapabilities(false, emptyList(), 24_000)
            override suspend fun synthesize(
                text: String, voice: String?, keyProvider: suspend () -> String?,
            ): Flow<ByteArray> = flow { throw SpeechFfiException.Unavailable() }
        }
        val e = runCatching { AndroidTtsAdapter(failing).synthesize("x", null) }.exceptionOrNull()
        assertTrue(e is SpeechFfiException.Unavailable)
    }

    @Test
    fun tts_wrapsArbitraryThrowable_inOther_withMessage() = runTest {
        val failing = object : TtsProvider {
            override val id = "boom"
            override val capabilities = TtsCapabilities(false, emptyList(), 24_000)
            override suspend fun synthesize(
                text: String, voice: String?, keyProvider: suspend () -> String?,
            ): Flow<ByteArray> = flow { throw IllegalStateException("engine died") }
        }
        val e = runCatching { AndroidTtsAdapter(failing).synthesize("x", null) }.exceptionOrNull()
        assertTrue(e is SpeechFfiException.Other)
        assertEquals("engine died", (e as SpeechFfiException.Other).message)
    }
}
