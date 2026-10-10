package com.lingxi.code.settings

import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioConfigurationRepositoryTest {
    @Test
    fun emptyStoreUsesCurrentDefaultsWithoutWritingOrAdvancingRevision() {
        val storage = MemoryStorage()
        val repository = AudioConfigurationRepository(storage)
        assertEquals(VersionedAudioConfiguration(AudioConfigurationNormalizer.defaults, 0L), repository.load().snapshot)
        assertEquals(0, storage.writeCount)
        val saved = repository.save(AudioConfigurationNormalizer.defaults.copy(language = "en-US"), 0L)
        assertTrue(saved is AudioConfigurationSaveResult.Saved)
        assertEquals(1L, (saved as AudioConfigurationSaveResult.Saved).snapshot.revision)
        assertEquals(4, (storage.configuration as Map<*, *>)["schemaVersion"])
    }

    @Test
    fun nonCurrentConfigurationIsIgnoredWithoutReadingItsFieldsOrRewritingIt() {
        val raw = mapOf("schemaVersion" to 3, "language" to "fr-FR", "rate" to 1.5,
            "recognition" to mapOf("source" to "offline", "offlineModelId" to "previous-model"))
        val storage = MemoryStorage(configuration = raw, revision = 7L)
        val repository = AudioConfigurationRepository(storage)
        assertEquals(VersionedAudioConfiguration(AudioConfigurationNormalizer.defaults, 7L), repository.load().snapshot)
        assertEquals(raw, storage.configuration)
        assertEquals(0, storage.writeCount)
    }

    private class MemoryStorage(
        var configuration: Any? = null,
        var revision: Long = 0,
        var writesSucceed: Boolean = true,
        var mutateBeforeFailedWrite: Boolean = false,
    ) : AudioConfigurationStorage {
        var writeCount = 0

        override fun readConfiguration(): Any? = configuration
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
            AudioConfigurationV4(
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

    private fun AudioConfigurationV4.toMapForTest(): Map<String, Any?> = toStorageMap()
}
