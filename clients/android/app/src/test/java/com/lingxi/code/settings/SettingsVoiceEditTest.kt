package com.lingxi.code.settings

import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV3
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@OptIn(ExperimentalCoroutinesApi::class)
class SettingsVoiceEditTest {
    @Test
    fun rapidEditsApplyToTheLatestConfigurationAfterTheFirstSaveCompletes() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        val firstWriteEntered = CountDownLatch(1)
        val allowFirstWrite = CountDownLatch(1)
        val firstWriteCommitted = CountDownLatch(1)
        val secondWriteCommitted = CountDownLatch(1)
        val storage = BlockingVoiceStorage(firstWriteEntered, allowFirstWrite, firstWriteCommitted, secondWriteCommitted)
        val store = SettingsStore(voiceRepo = AudioConfigurationRepository(storage))
        try {
            store.updateVoice { it.copy(language = "en-US") }
            store.updateVoice { it.copy(rate = 1.5) }
            runCurrent()

            assertTrue("first save should be blocked before the second edit is applied", firstWriteEntered.await(2, TimeUnit.SECONDS))
            assertEquals("auto", store.state.value.voice.language)
            assertEquals(1.0, store.state.value.voice.rate, 0.0)

            allowFirstWrite.countDown()
            assertTrue(firstWriteCommitted.await(2, TimeUnit.SECONDS))
            val firstCommitDeadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
            while (store.state.value.voiceRevision < 2L && System.nanoTime() < firstCommitDeadline) {
                runCurrent()
                Thread.sleep(5)
            }
            assertEquals(
                "first commit: revision=${store.state.value.voiceRevision}, saving=${store.state.value.voiceSaving}, error=${store.state.value.voiceSaveError}",
                "en-US",
                store.state.value.voice.language,
            )

            assertTrue("queued edit should reach its own storage commit", secondWriteCommitted.await(2, TimeUnit.SECONDS))
            val secondCommitDeadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
            while (store.state.value.voiceRevision < 3L && System.nanoTime() < secondCommitDeadline) {
                runCurrent()
                Thread.sleep(5)
            }
            advanceUntilIdle()

            assertEquals("persisted patch should retain the previous language edit", "en-US", AudioConfigurationNormalizer.normalize(storage.readConfiguration()).language)
            assertEquals("en-US", store.state.value.voice.language)
            assertEquals(1.5, store.state.value.voice.rate, 0.0)
            assertEquals(3L, store.state.value.voiceRevision)
            assertTrue(!store.state.value.voiceSaving)
        } finally {
            allowFirstWrite.countDown()
            Dispatchers.resetMain()
        }
    }

    private class BlockingVoiceStorage(
        private val firstWriteEntered: CountDownLatch,
        private val allowFirstWrite: CountDownLatch,
        private val firstWriteCommitted: CountDownLatch,
        private val secondWriteCommitted: CountDownLatch,
    ) : AudioConfigurationStorage {
        private val lock = Any()
        private var configuration: Any? = audioConfigurationMap(AudioConfigurationNormalizer.defaults)
        @Volatile var revision: Long = 1L
            private set
        private var writes = 0

        override fun readConfiguration(): Any? = synchronized(lock) { configuration }

        override fun readLegacyConfiguration(): Map<String, Any?> = emptyMap()

        override fun readRevision(): Long = revision

        override fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean {
            val writeNumber = synchronized(lock) { ++writes }
            val shouldBlock = writeNumber == 1
            if (shouldBlock) {
                firstWriteEntered.countDown()
                if (!allowFirstWrite.await(3, TimeUnit.SECONDS)) return false
            }
            synchronized(lock) {
                configuration = value
                this.revision = revision
            }
            if (writeNumber == 1) firstWriteCommitted.countDown()
            if (writeNumber == 2) secondWriteCommitted.countDown()
            return true
        }
    }

}

private fun audioConfigurationMap(configuration: AudioConfigurationV3): Map<String, Any?> = mapOf(
        "schemaVersion" to configuration.schemaVersion,
        "recognition" to mapOf(
            "source" to configuration.recognition.source.value,
            "offlineModelId" to configuration.recognition.offlineModelId,
        ),
        "speech" to mapOf(
            "source" to configuration.speech.source.value,
            "offlineModelId" to configuration.speech.offlineModelId,
            "voice" to null,
        ),
        "language" to configuration.language,
        "rate" to configuration.rate,
        "autoPlayReplies" to configuration.autoPlayReplies,
    )
