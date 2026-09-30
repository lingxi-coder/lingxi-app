package com.lingxi.code.voice.audio

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioConfigurationFixtureTest {
    private val fixtures: Map<String, Any?> by lazy {
        val json = checkNotNull(javaClass.classLoader?.getResourceAsStream("audio-config-fixtures.json")) {
            "shared audio config fixtures were not added to Android test resources"
        }.bufferedReader().use { it.readText() }
        JSONObject(json).toFixtureMap()
    }

    @Test
    fun generatedNormalizerMatchesSharedNormalizationAndMigrationFixtures() {
        val cases = fixtures["normalization"] as List<Map<String, Any?>> +
            (fixtures["migrations"] as List<Map<String, Any?>>)

        cases.forEach { testCase ->
            val actual = if (testCase in (fixtures["normalization"] as List<*>)) {
                AudioConfigurationNormalizer.normalize(testCase["input"])
            } else {
                AudioConfigurationNormalizer.migrateLegacy(testCase["input"])
            }
            assertEquals(
                "shared config fixture '${testCase["name"]}'",
                normalizeJsonNumbers(testCase["expected"]),
                normalizeJsonNumbers(actual.toFixtureMap()),
            )
        }
    }

    @Test
    fun generatedRouteResolverMatchesSharedRouteFixtures() {
        val cases = fixtures["routes"] as List<Map<String, Any?>>
        cases.forEach { testCase ->
            val input = testCase["input"] as Map<String, Any?>
            val preference = input["preference"] as Map<String, Any?>
            val kind = AudioProviderKind.valueOf((input["kind"] as String).uppercase())
            val source = AudioSource(preference["source"] as String)
            val voice = (preference["voice"] as? Map<String, Any?>)?.toVoiceSelection()
                ?: (input["voiceOverride"] as? Map<String, Any?>)?.toVoiceSelection()
            val models = (input["offlineModels"] as List<Map<String, Any?>>).map { model ->
                AudioOfflineModelAvailability(
                    id = model["id"] as String,
                    kind = AudioProviderKind.valueOf((model["kind"] as String).uppercase()),
                    languages = model["languages"] as List<String>,
                    installed = model["installed"] as Boolean,
                    voiceIds = model["voiceIds"] as? List<String>,
                )
            }
            val request = AudioRouteRequest(
                kind = kind,
                source = source,
                offlineModelId = preference["offlineModelId"] as? String,
                voice = voice,
                language = input["language"] as String,
                systemStatus = AudioReadiness.valueOf((input["systemStatus"] as String).uppercase()),
                offlineModels = models,
                systemVoiceIds = input["systemVoiceIds"] as? List<String>,
            )

            val actual = resolveAudioRoute(request).toFixtureMap()
            assertEquals(
                "shared audio route fixture '${testCase["name"]}'",
                normalizeJsonNumbers(testCase["expected"]),
                normalizeJsonNumbers(actual),
            )
        }
    }

    @Test
    fun automaticFallbackIsLimitedToPreStartPermissionAndUnavailability() {
        assertTrue(isAudioFallbackAllowed(AudioFallbackFailure.PERMISSION, operationStarted = false))
        assertTrue(isAudioFallbackAllowed(AudioFallbackFailure.UNAVAILABLE, operationStarted = false))
        assertFalse(isAudioFallbackAllowed(AudioFallbackFailure.PERMISSION, operationStarted = true))
        listOf(
            AudioFallbackFailure.BUSY,
            AudioFallbackFailure.CANCELLED,
            AudioFallbackFailure.TIMEOUT,
            AudioFallbackFailure.INVALID_REQUEST,
            AudioFallbackFailure.NO_SPEECH,
            AudioFallbackFailure.NATIVE_FAILURE,
        ).forEach { failure ->
            assertFalse("$failure cannot trigger fallback", isAudioFallbackAllowed(failure, operationStarted = false))
        }
    }

    private fun Map<String, Any?>.toVoiceSelection(): AudioVoiceSelection = AudioVoiceSelection(
        source = AudioSource(this["source"] as String),
        id = this["id"] as String,
        modelId = this["modelId"] as? String,
    )

    private fun AudioConfigurationV3.toFixtureMap(): Map<String, Any?> = mapOf(
        "schemaVersion" to schemaVersion,
        "recognition" to mapOf(
            "source" to recognition.source.value,
            "offlineModelId" to recognition.offlineModelId,
        ),
        "speech" to mapOf(
            "source" to speech.source.value,
            "offlineModelId" to speech.offlineModelId,
            "voice" to speech.voice?.toFixtureMap(),
        ),
        "language" to language,
        "rate" to rate,
        "autoPlayReplies" to autoPlayReplies,
    )

    private fun AudioVoiceSelection.toFixtureMap(): Map<String, Any?> = buildMap {
        put("source", source.value)
        modelId?.let { put("modelId", it) }
        put("id", id)
    }

    private fun AudioRouteResolution.toFixtureMap(): Map<String, Any?> = mapOf(
        "requested" to mapOf(
            "source" to requested.source.value,
            "offlineModelId" to requested.offlineModelId,
            "voice" to requested.voice?.toFixtureMap(),
        ),
        "effective" to effective?.let {
            mapOf(
                "source" to it.source.value,
                "modelId" to it.modelId,
                "voiceId" to it.voiceId,
            )
        },
        "status" to when (status) {
            AudioRouteStatus.READY -> "ready"
            AudioRouteStatus.PERMISSION_REQUIRED -> "permissionRequired"
            AudioRouteStatus.UNAVAILABLE -> "unavailable"
            AudioRouteStatus.INVALID_REQUEST -> "invalidRequest"
        },
        "reason" to reason,
        "fallbackReason" to fallbackReason,
    )

    private fun JSONObject.toFixtureMap(): Map<String, Any?> =
        keys().asSequence().associateWith { key -> get(key).toFixtureValue() }

    private fun Any?.toFixtureValue(): Any? = when (this) {
        JSONObject.NULL -> null
        is JSONObject -> toFixtureMap()
        is JSONArray -> (0 until length()).map { get(it).toFixtureValue() }
        else -> this
    }

    private fun normalizeJsonNumbers(value: Any?): Any? = when (value) {
        is Map<*, *> -> value.entries.associate { it.key to normalizeJsonNumbers(it.value) }
        is List<*> -> value.map(::normalizeJsonNumbers)
        is Number -> value.toDouble()
        else -> value
    }
}
