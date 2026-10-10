package com.lingxi.code.settings

import android.content.Context
import android.content.SharedPreferences
import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV4
import com.lingxi.code.voice.audio.AudioVoiceSelection
import org.json.JSONArray
import org.json.JSONObject

data class VersionedAudioConfiguration(
    val configuration: AudioConfigurationV4,
    val revision: Long,
)

data class AudioConfigurationLoadResult(
    val snapshot: VersionedAudioConfiguration,
    val persistenceError: String? = null,
)

sealed interface AudioConfigurationSaveResult {
    data class Saved(val snapshot: VersionedAudioConfiguration) : AudioConfigurationSaveResult
    data class Conflict(val current: VersionedAudioConfiguration) : AudioConfigurationSaveResult
    data class Failed(val message: String) : AudioConfigurationSaveResult
}

/** Storage seam keeps durable revision behavior testable without Android preferences. */
internal interface AudioConfigurationStorage {
    fun readConfiguration(): Any?
    fun readRevision(): Long
    fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean
}

/** Read marker for malformed persisted data. Callers must preserve the raw value and fail routes closed. */
internal data class CorruptAudioConfiguration(val message: String)

/** Device-local v4 store. Writes are serialized and advance revision only after durable commit. */
class AudioConfigurationRepository internal constructor(
    private val storage: AudioConfigurationStorage,
    authorityKey: Any = storage,
) {
    constructor(context: Context) : this(
        storage = SharedPreferencesAudioConfigurationStorage(context),
        authorityKey = context.applicationContext,
    )

    private val sharedState = synchronized(PROCESS_LOCK) {
        REPOSITORY_STATES.getOrPut(authorityKey) { RepositoryState() }
    }

    fun load(): AudioConfigurationLoadResult = synchronized(PROCESS_LOCK) {
        if (sharedState.hasUncommittedWriteFailure) {
            sharedState.lastCommittedSnapshot?.let { snapshot ->
                return@synchronized AudioConfigurationLoadResult(
                    snapshot = snapshot,
                    persistenceError = "Audio settings could not be saved. The last saved settings are still active.",
                )
            }
        }
        val raw = storage.readConfiguration()
        val revision = storage.readRevision().coerceAtLeast(0L)
        if (raw is CorruptAudioConfiguration) {
            val snapshot = sharedState.lastCommittedSnapshot?.takeIf { it.revision == revision }
                ?: VersionedAudioConfiguration(unavailableConfiguration(), revision)
            if (sharedState.lastCommittedSnapshot == null) sharedState.lastCommittedSnapshot = snapshot
            return@synchronized AudioConfigurationLoadResult(
                snapshot = snapshot,
                persistenceError = "Audio settings data is invalid and was preserved. Select audio sources again to replace it.",
            )
        }
        val snapshot = VersionedAudioConfiguration(AudioConfigurationNormalizer.normalize(raw), revision)
        sharedState.lastCommittedSnapshot = snapshot
        sharedState.hasUncommittedWriteFailure = false
        AudioConfigurationLoadResult(snapshot)
    }

    fun save(
        configuration: AudioConfigurationV4,
        expectedRevision: Long,
    ): AudioConfigurationSaveResult = synchronized(PROCESS_LOCK) {
        val currentRevision = if (sharedState.hasUncommittedWriteFailure) {
            sharedState.lastCommittedSnapshot?.revision ?: storage.readRevision().coerceAtLeast(0L)
        } else {
            storage.readRevision().coerceAtLeast(0L)
        }
        if (currentRevision != expectedRevision) {
            val current = if (sharedState.hasUncommittedWriteFailure) {
                sharedState.lastCommittedSnapshot ?: readCurrentSnapshot(currentRevision)
            } else {
                readCurrentSnapshot(currentRevision)
            }
            return@synchronized AudioConfigurationSaveResult.Conflict(current)
        }
        val normalized = AudioConfigurationNormalizer.normalize(configuration.toStorageMap())
        val nextRevision = nextRevision(currentRevision)
            ?: return@synchronized AudioConfigurationSaveResult.Failed("Audio settings revision is exhausted.")
        if (sharedState.lastCommittedSnapshot == null) {
            sharedState.lastCommittedSnapshot = readCurrentSnapshot(currentRevision)
        }
        if (!storage.writeConfiguration(normalized.toStorageMap(), nextRevision)) {
            sharedState.hasUncommittedWriteFailure = true
            return@synchronized AudioConfigurationSaveResult.Failed("Audio settings could not be saved.")
        }
        sharedState.hasUncommittedWriteFailure = false
        val snapshot = VersionedAudioConfiguration(normalized, nextRevision)
        sharedState.lastCommittedSnapshot = snapshot
        AudioConfigurationSaveResult.Saved(snapshot)
    }

    private fun readCurrentSnapshot(revision: Long): VersionedAudioConfiguration {
        val raw = storage.readConfiguration()
        val configuration = if (raw is CorruptAudioConfiguration) {
            sharedState.lastCommittedSnapshot?.configuration ?: unavailableConfiguration()
        } else {
            AudioConfigurationNormalizer.normalize(raw)
        }
        return VersionedAudioConfiguration(configuration, revision)
    }

    private fun nextRevision(current: Long): Long? = (current + 1L).takeIf { current < MAX_REVISION }

    private companion object {
        const val MAX_REVISION = 9_007_199_254_740_991L
        val PROCESS_LOCK = Any()
        val REPOSITORY_STATES = java.util.WeakHashMap<Any, RepositoryState>()
    }

    private class RepositoryState(
        var lastCommittedSnapshot: VersionedAudioConfiguration? = null,
        var hasUncommittedWriteFailure: Boolean = false,
    )
}

private class SharedPreferencesAudioConfigurationStorage(context: Context) : AudioConfigurationStorage {
    private val appContext = context.applicationContext
    private val preferences: SharedPreferences = appContext.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    override fun readConfiguration(): Any? = preferences.getString(KEY_CONFIGURATION, null)?.let { value ->
        runCatching { parseJson(value) }.getOrElse { CorruptAudioConfiguration("Audio settings are malformed.") }
    }

    override fun readRevision(): Long = preferences.getLong(KEY_REVISION, 0L)

    override fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean =
        preferences.edit()
            .putString(KEY_CONFIGURATION, JSONObject(value).toString())
            .putLong(KEY_REVISION, revision)
            .commit()

    private companion object {
        const val PREFERENCES_NAME = "voice_settings"
        const val KEY_CONFIGURATION = "audio_configuration_v4"
        const val KEY_REVISION = "audio_configuration_revision"
    }
}

internal fun AudioConfigurationV4.toStorageMap(): Map<String, Any?> = mapOf(
    "schemaVersion" to 4,
    "recognition" to mapOf(
        "source" to recognition.source.value,
        "offlineModelId" to recognition.offlineModelId,
        "cloud" to recognition.cloud.toStorageMap(),
    ),
    "speech" to mapOf(
        "source" to speech.source.value,
        "offlineModelId" to speech.offlineModelId,
        "voice" to speech.voice?.toStorageMap(),
        "cloud" to speech.cloud.toStorageMap(),
    ),
    "conversation" to mapOf(
        "mode" to conversation.mode,
        "interaction" to conversation.interaction,
        "cloud" to conversation.cloud.toStorageMap(),
        "voice" to conversation.voice?.toStorageMap(),
    ),
    "language" to language,
    "rate" to rate,
    "autoPlayReplies" to autoPlayReplies,
)

private fun unavailableConfiguration(): AudioConfigurationV4 = AudioConfigurationV4(
    recognition = com.lingxi.code.voice.audio.AudioRecognitionPreference(
        source = com.lingxi.code.voice.audio.AudioSource("invalidStoredConfiguration"),
    ),
    speech = com.lingxi.code.voice.audio.AudioSpeechPreference(
        source = com.lingxi.code.voice.audio.AudioSource("invalidStoredConfiguration"),
    ),
)

private fun AudioVoiceSelection.toStorageMap(): Map<String, Any?> = buildMap {
    put("source", source.value)
    put("id", id)
    modelId?.let { put("modelId", it) }
    profileId?.let { put("profileId", it) }
}

private fun parseJson(text: String): Map<String, Any?> = JSONObject(text).toStringMap()

private fun JSONObject.toStringMap(): Map<String, Any?> = keys().asSequence().associateWith { key ->
    get(key).toPlainValue()
}

private fun Any?.toPlainValue(): Any? = when (this) {
    JSONObject.NULL -> null
    is JSONObject -> toStringMap()
    is JSONArray -> (0 until length()).map { get(it).toPlainValue() }
    else -> this
}

private fun com.lingxi.code.voice.audio.AudioCloudBinding.toStorageMap(): Map<String, Any?> = mapOf(
    "binding" to binding, "profileId" to profileId, "modelId" to modelId,
)
