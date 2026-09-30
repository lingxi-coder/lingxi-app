package com.lingxi.code.settings

import android.content.Context
import android.content.SharedPreferences
import com.lingxi.code.theme.AppearanceStore
import com.lingxi.code.voice.audio.AudioConfigurationNormalizer
import com.lingxi.code.voice.audio.AudioConfigurationV3
import com.lingxi.code.voice.audio.AudioVoiceSelection
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject

data class VersionedAudioConfiguration(
    val configuration: AudioConfigurationV3,
    val revision: Long,
)

data class AudioConfigurationLoadResult(
    val snapshot: VersionedAudioConfiguration,
    val migrated: Boolean,
    val persistenceError: String? = null,
)

sealed interface AudioConfigurationSaveResult {
    data class Saved(val snapshot: VersionedAudioConfiguration) : AudioConfigurationSaveResult
    data class Conflict(val current: VersionedAudioConfiguration) : AudioConfigurationSaveResult
    data class Failed(val message: String) : AudioConfigurationSaveResult
}

/** Storage seam keeps revision and migration behavior testable without Android preferences. */
internal interface AudioConfigurationStorage {
    fun readConfiguration(): Any?
    fun readLegacyConfiguration(): Map<String, Any?>
    fun readRevision(): Long
    fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean
}

/** Read marker for malformed persisted data. Callers must preserve the raw value and fail routes closed. */
internal data class CorruptAudioConfiguration(val message: String)

/** Device-local v3 store. Writes are serialized and advance revision only after durable commit. */
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
        val raw = storage.readConfiguration()
        if (sharedState.hasUncommittedWriteFailure) {
            val committed = sharedState.lastCommittedSnapshot
            if (sharedState.retryMigrationAfterFailure) {
                val pendingMigration = sharedState.pendingMigrationConfiguration
                val baseRevision = committed?.revision ?: storage.readRevision().coerceAtLeast(0L)
                val nextRevision = nextRevision(baseRevision)
                if (pendingMigration != null && nextRevision != null &&
                    storage.writeConfiguration(pendingMigration.toStorageMap(), nextRevision)
                ) {
                    val migrated = VersionedAudioConfiguration(pendingMigration, nextRevision)
                    sharedState.lastCommittedSnapshot = migrated
                    sharedState.hasUncommittedWriteFailure = false
                    sharedState.retryMigrationAfterFailure = false
                    sharedState.pendingMigrationConfiguration = null
                    return@synchronized AudioConfigurationLoadResult(snapshot = migrated, migrated = true)
                }
                val fallback = committed ?: VersionedAudioConfiguration(unavailableConfiguration(), baseRevision)
                return@synchronized AudioConfigurationLoadResult(
                    snapshot = fallback,
                    migrated = false,
                    persistenceError = if (nextRevision == null) {
                        "Audio settings revision is exhausted; migration was not saved."
                    } else {
                        "Audio settings migration could not be saved."
                    },
                )
            }
            committed?.let { snapshot ->
                return@synchronized AudioConfigurationLoadResult(
                    snapshot = snapshot,
                    migrated = false,
                    persistenceError = "Audio settings could not be saved. The last saved settings are still active.",
                )
            }
        }
        val revision = storage.readRevision().coerceAtLeast(0L)
        if (raw is CorruptAudioConfiguration) {
            val snapshot = sharedState.lastCommittedSnapshot?.takeIf { it.revision == revision }
                ?: VersionedAudioConfiguration(unavailableConfiguration(), revision)
            if (sharedState.lastCommittedSnapshot == null) sharedState.lastCommittedSnapshot = snapshot
            return@synchronized AudioConfigurationLoadResult(
                snapshot = snapshot,
                migrated = false,
                persistenceError = "Audio settings data is invalid and was preserved. Select audio sources again to replace it.",
            )
        }
        if (raw == null) {
            val migrated = AudioConfigurationNormalizer.migrateLegacy(storage.readLegacyConfiguration())
            val nextRevision = nextRevision(revision)
            if (nextRevision == null) {
                val snapshot = VersionedAudioConfiguration(migrated, revision)
                sharedState.lastCommittedSnapshot = snapshot
                return@synchronized AudioConfigurationLoadResult(
                    snapshot = snapshot,
                    migrated = false,
                    persistenceError = "Audio settings revision is exhausted; migration was not saved.",
                )
            }
            val migrationSnapshot = VersionedAudioConfiguration(migrated, revision)
            sharedState.lastCommittedSnapshot = migrationSnapshot
            if (!storage.writeConfiguration(migrated.toStorageMap(), nextRevision)) {
                sharedState.hasUncommittedWriteFailure = true
                sharedState.retryMigrationAfterFailure = true
                sharedState.pendingMigrationConfiguration = migrated
                return@synchronized AudioConfigurationLoadResult(
                    snapshot = migrationSnapshot,
                    migrated = false,
                    persistenceError = "Audio settings migration could not be saved.",
                )
            }
            val migratedSnapshot = VersionedAudioConfiguration(migrated, nextRevision)
            sharedState.lastCommittedSnapshot = migratedSnapshot
            sharedState.hasUncommittedWriteFailure = false
            sharedState.retryMigrationAfterFailure = false
            sharedState.pendingMigrationConfiguration = null
            return@synchronized AudioConfigurationLoadResult(snapshot = migratedSnapshot, migrated = true)
        }

        val normalized = AudioConfigurationNormalizer.normalize(raw)
        val snapshot = VersionedAudioConfiguration(normalized, revision)
        sharedState.lastCommittedSnapshot = snapshot
        sharedState.hasUncommittedWriteFailure = false
        sharedState.retryMigrationAfterFailure = false
        sharedState.pendingMigrationConfiguration = null
        AudioConfigurationLoadResult(
            snapshot,
            migrated = false,
        )
    }

    fun save(
        configuration: AudioConfigurationV3,
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
            sharedState.retryMigrationAfterFailure = false
            sharedState.pendingMigrationConfiguration = null
            return@synchronized AudioConfigurationSaveResult.Failed("Audio settings could not be saved.")
        }
        sharedState.hasUncommittedWriteFailure = false
        sharedState.retryMigrationAfterFailure = false
        sharedState.pendingMigrationConfiguration = null
        val snapshot = VersionedAudioConfiguration(normalized, nextRevision)
        sharedState.lastCommittedSnapshot = snapshot
        AudioConfigurationSaveResult.Saved(snapshot)
    }

    private fun readCurrentSnapshot(revision: Long): VersionedAudioConfiguration {
        val raw = storage.readConfiguration()
        val configuration = if (raw is CorruptAudioConfiguration) {
            sharedState.lastCommittedSnapshot?.configuration ?: unavailableConfiguration()
        } else if (raw == null) {
            AudioConfigurationNormalizer.migrateLegacy(storage.readLegacyConfiguration())
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
        var retryMigrationAfterFailure: Boolean = false,
        var pendingMigrationConfiguration: AudioConfigurationV3? = null,
    )
}

private class SharedPreferencesAudioConfigurationStorage(context: Context) : AudioConfigurationStorage {
    private val appContext = context.applicationContext
    private val preferences: SharedPreferences = appContext.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    override fun readConfiguration(): Any? = preferences.getString(KEY_CONFIGURATION, null)?.let { value ->
        runCatching { parseJson(value) }.getOrElse { CorruptAudioConfiguration("Audio settings are malformed.") }
    }

    override fun readLegacyConfiguration(): Map<String, Any?> {
        val oldSchema = preferences.getInt(KEY_SCHEMA_VERSION, 0)
        val voiceLanguage = if (oldSchema < 3) {
            runCatching { runBlocking { AppearanceStore(appContext).prefs.first().voiceLang } }.getOrNull()
        } else {
            null
        }
        return buildMap {
            put("schemaVersion", oldSchema)
            put("recognitionMode", preferences.getString("recognition_mode", null))
            put("language", preferences.getString("language", null))
            put("voiceSelection", preferences.getString("voice_selection", null))
            if (preferences.contains("rate")) put("rate", preferences.getFloat("rate", 1.0f))
            if (preferences.contains("auto_play_replies")) put("autoPlayReplies", preferences.getBoolean("auto_play_replies", false))
            put("inputProvider", preferences.getString("input_provider", null))
            put("inputLanguage", preferences.getString("input_language", null))
            put("outputProvider", preferences.getString("output_provider", null))
            put("voice", preferences.getString("voice", null))
            if (preferences.contains("speed")) put("speed", preferences.getFloat("speed", 1.0f))
            if (preferences.contains("auto_play")) put("autoPlay", preferences.getBoolean("auto_play", false))
            voiceLanguage?.let { put("legacyVoiceLang", it) }
        }
    }

    override fun readRevision(): Long = preferences.getLong(KEY_REVISION, 0L)

    override fun writeConfiguration(value: Map<String, Any?>, revision: Long): Boolean =
        preferences.edit()
            .putString(KEY_CONFIGURATION, JSONObject(value).toString())
            .putInt(KEY_SCHEMA_VERSION, 3)
            .putLong(KEY_REVISION, revision)
            .commit()

    private companion object {
        const val PREFERENCES_NAME = "voice_settings"
        const val KEY_CONFIGURATION = "audio_configuration_v3"
        const val KEY_REVISION = "audio_configuration_revision"
        const val KEY_SCHEMA_VERSION = "schema_version"
    }
}

private fun AudioConfigurationV3.toStorageMap(): Map<String, Any?> = mapOf(
    "schemaVersion" to 3,
    "recognition" to mapOf(
        "source" to recognition.source.value,
        "offlineModelId" to recognition.offlineModelId,
    ),
    "speech" to mapOf(
        "source" to speech.source.value,
        "offlineModelId" to speech.offlineModelId,
        "voice" to speech.voice?.toStorageMap(),
    ),
    "language" to language,
    "rate" to rate,
    "autoPlayReplies" to autoPlayReplies,
)

private fun unavailableConfiguration(): AudioConfigurationV3 = AudioConfigurationV3(
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
