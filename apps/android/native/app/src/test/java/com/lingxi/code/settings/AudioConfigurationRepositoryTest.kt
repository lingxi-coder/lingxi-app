package com.lingxi.code.settings

import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV3
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioConfigurationRepositoryTest {
    private class MemoryStorage(
        var configuration: Any? = null,
        val legacy: Map<String, Any?> = emptyMap(),
        var revision: Long = 0,
        var writesSucceed: Boolean = true,
        var mutateBeforeFailedWrite: Boolean = false,
    ) : AudioConfigurationStorage {
        var writeCount = 0

        override fun readConfiguration(): Any? = configuration
        override fun readLegacyConfiguration(): Map<String, Any?> = legacy
        override fun readRevision(): Long = revision

        override fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean {
            writeCount++
            if (!writesSucceed) {
                if (mutateBeforeFailedWrite) {
                    configuration = value
                    this.revision = revision
                }
                return false
            }
            configuration = value
            this.revision = revision
            return true
        }
    }

    @Test
    fun migrationIsPersistedOnceAndKeepsExplicitOfflineSelection() {
        val storage = MemoryStorage(
            legacy = mapOf(
                "schemaVersion" to 2,
                "recognitionMode" to "localOnly",
                "language" to " zh-CN ",
                "voiceSelection" to "sherpa:sherpa.melo-zh-en:melo-zh-en",
                "autoPlayReplies" to true,
            ),
        )
        val repository = AudioConfigurationRepository(storage)

        val first = repository.load()
        val second = repository.load()

        assertTrue(first.migrated)
        assertEquals(1L, first.snapshot.revision)
        assertEquals(AudioSource.OFFLINE, first.snapshot.configuration.recognition.source)
        assertEquals("zh-CN", first.snapshot.configuration.language)
        assertEquals(AudioSource.OFFLINE, first.snapshot.configuration.speech.source)
        assertEquals("sherpa.melo-zh-en", first.snapshot.configuration.speech.offlineModelId)
        assertTrue(first.snapshot.configuration.autoPlayReplies)
        assertFalse(second.migrated)
        assertEquals(first.snapshot, second.snapshot)
        assertEquals(1, storage.writeCount)
    }

    @Test
    fun failedMigrationRemainsRetryableAndDoesNotAdvanceRevision() {
        val storage = MemoryStorage(
            legacy = mapOf("schemaVersion" to 2, "recognitionMode" to "localOnly"),
            writesSucceed = false,
        )
        val repository = AudioConfigurationRepository(storage)

        val failed = repository.load()

        assertFalse(failed.migrated)
        assertEquals(0L, failed.snapshot.revision)
        assertEquals("Audio settings migration could not be saved.", failed.persistenceError)
        assertNull(storage.configuration)

        storage.writesSucceed = true
        val retry = repository.load()

        assertTrue(retry.migrated)
        assertEquals(1L, retry.snapshot.revision)
        assertEquals(2, storage.writeCount)
    }

    @Test
    fun failedMigrationCommitThatMutatesPreferencesMemoryRetriesFromCommittedRevision() {
        val storage = MemoryStorage(
            legacy = mapOf("schemaVersion" to 2, "recognitionMode" to "localOnly"),
            writesSucceed = false,
            mutateBeforeFailedWrite = true,
        )
        val repository = AudioConfigurationRepository(storage)

        val failed = repository.load()
        assertFalse(failed.migrated)
        assertEquals(0L, failed.snapshot.revision)
        assertEquals(1L, storage.revision)
        assertTrue(storage.configuration != null)

        storage.writesSucceed = true
        val retry = repository.load()

        assertTrue("the migration must be durably retried, not inferred from mutated memory", retry.migrated)
        assertEquals(1L, retry.snapshot.revision)
        assertEquals(AudioSource.OFFLINE, retry.snapshot.configuration.recognition.source)
        assertEquals(2, storage.writeCount)
    }

    @Test
    fun staleRevisionReturnsCurrentSnapshotWithoutWriting() {
        val configuration = AudioConfigurationNormalizer.defaults
        val storage = MemoryStorage(
            configuration = configuration.toMapForTest(),
            revision = 8,
        )
        val repository = AudioConfigurationRepository(storage)

        val result = repository.save(
            configuration.copy(language = "fr-FR"),
            expectedRevision = 7,
        )

        assertTrue(result is AudioConfigurationSaveResult.Conflict)
        assertEquals(8L, (result as AudioConfigurationSaveResult.Conflict).current.revision)
        assertEquals("auto", result.current.configuration.language)
        assertEquals(0, storage.writeCount)
    }

    @Test
    fun failedSaveLeavesStoredConfigurationAndRevisionUntouched() {
        val current = AudioConfigurationNormalizer.defaults
        val storage = MemoryStorage(
            configuration = current.toMapForTest(),
            revision = 4,
            writesSucceed = false,
        )
        val repository = AudioConfigurationRepository(storage)

        val result = repository.save(current.copy(language = "en-US"), expectedRevision = 4)

        assertTrue(result is AudioConfigurationSaveResult.Failed)
        assertEquals(4L, storage.revision)
        assertEquals("auto", AudioConfigurationNormalizer.normalize(storage.configuration).language)
    }

    @Test
    fun failedCommitThatMutatesSharedPreferencesMemoryKeepsLastCommittedSnapshotAndRevision() {
        val initial = AudioConfigurationNormalizer.defaults
        val storage = MemoryStorage(
            configuration = initial.toMapForTest(),
            revision = 4,
            writesSucceed = false,
            mutateBeforeFailedWrite = true,
        )
        val repository = AudioConfigurationRepository(storage)
        val firstLoad = repository.load()

        val failed = repository.save(initial.copy(language = "fr-FR"), expectedRevision = firstLoad.snapshot.revision)
        assertTrue(failed is AudioConfigurationSaveResult.Failed)
        assertEquals(5L, storage.revision)

        val reloaded = repository.load()
        assertEquals(4L, reloaded.snapshot.revision)
        assertEquals("auto", reloaded.snapshot.configuration.language)
        assertTrue(reloaded.persistenceError != null)

        storage.writesSucceed = true
        val retry = repository.save(initial.copy(language = "de-DE"), expectedRevision = reloaded.snapshot.revision)
        assertTrue(retry is AudioConfigurationSaveResult.Saved)
        assertEquals(5L, (retry as AudioConfigurationSaveResult.Saved).snapshot.revision)
        assertEquals("de-DE", AudioConfigurationNormalizer.normalize(storage.configuration).language)
    }

    @Test
    fun failedMutatingCommitIsSharedAcrossRepositoryInstances() {
        val initial = AudioConfigurationNormalizer.defaults
        val storage = MemoryStorage(
            configuration = initial.toMapForTest(),
            revision = 12,
            writesSucceed = false,
            mutateBeforeFailedWrite = true,
        )
        val settingsRepository = AudioConfigurationRepository(storage)
        val runtimeRepository = AudioConfigurationRepository(storage)
        val current = settingsRepository.load().snapshot

        val failed = settingsRepository.save(initial.copy(language = "fr-FR"), current.revision)
        assertTrue(failed is AudioConfigurationSaveResult.Failed)

        val runtimeSnapshot = runtimeRepository.load()
        assertEquals(current, runtimeSnapshot.snapshot)
        assertTrue(runtimeSnapshot.persistenceError != null)
        assertEquals("auto", runtimeSnapshot.snapshot.configuration.language)
        assertEquals(12L, runtimeSnapshot.snapshot.revision)
        val staleSave = runtimeRepository.save(initial.copy(language = "de-DE"), expectedRevision = 11L)
        assertTrue(staleSave is AudioConfigurationSaveResult.Conflict)
        val conflict = staleSave as AudioConfigurationSaveResult.Conflict
        assertEquals(12L, conflict.current.revision)
        assertEquals("auto", conflict.current.configuration.language)
    }

    @Test
    fun malformedStoredConfigurationIsPreservedAndRoutesFailClosed() {
        val corrupt = CorruptAudioConfiguration("malformed-json")
        val storage = MemoryStorage(configuration = corrupt, revision = 9)
        val repository = AudioConfigurationRepository(storage)

        val loaded = repository.load()

        assertEquals(corrupt, storage.configuration)
        assertEquals(0, storage.writeCount)
        assertTrue(loaded.persistenceError?.contains("preserved") == true)
        assertEquals(9L, loaded.snapshot.revision)
        assertEquals("invalidStoredConfiguration", loaded.snapshot.configuration.recognition.source.value)
        assertEquals("invalidStoredConfiguration", loaded.snapshot.configuration.speech.source.value)
    }

    @Test
    fun revisionExhaustionReturnsSaveFailureInsteadOfThrowing() {
        val storage = MemoryStorage(
            configuration = AudioConfigurationNormalizer.defaults.toMapForTest(),
            revision = 9_007_199_254_740_991L,
        )
        val repository = AudioConfigurationRepository(storage)

        val result = repository.save(AudioConfigurationNormalizer.defaults, expectedRevision = storage.revision)

        assertTrue(result is AudioConfigurationSaveResult.Failed)
        assertTrue((result as AudioConfigurationSaveResult.Failed).message.contains("exhausted"))
        assertEquals(0, storage.writeCount)
    }

    @Test
    fun successfulSaveNormalizesAndAdvancesRevision() {
        val storage = MemoryStorage(configuration = AudioConfigurationNormalizer.defaults.toMapForTest(), revision = 2)
        val repository = AudioConfigurationRepository(storage)

        val result = repository.save(
            AudioConfigurationV3(
                speech = AudioSpeechPreference(source = AudioSource.OFFLINE, offlineModelId = "missing-model"),
                language = " fr-FR ",
                rate = 4.0,
            ),
            expectedRevision = 2,
        )

        assertTrue(result is AudioConfigurationSaveResult.Saved)
        val saved = (result as AudioConfigurationSaveResult.Saved).snapshot
        assertEquals(3L, saved.revision)
        assertEquals("fr-FR", saved.configuration.language)
        assertEquals(2.0, saved.configuration.rate, 0.0)
        assertEquals("missing-model", saved.configuration.speech.offlineModelId)
    }

    private fun AudioConfigurationV3.toMapForTest(): Map<String, Any?> = mapOf(
        "schemaVersion" to schemaVersion,
        "recognition" to mapOf(
            "source" to recognition.source.value,
            "offlineModelId" to recognition.offlineModelId,
        ),
        "speech" to mapOf(
            "source" to speech.source.value,
            "offlineModelId" to speech.offlineModelId,
            "voice" to speech.voice?.let { mapOf("source" to it.source.value, "id" to it.id, "modelId" to it.modelId) },
        ),
        "language" to language,
        "rate" to rate,
        "autoPlayReplies" to autoPlayReplies,
    )
}
