package com.lingxi.code.voice.offline

fun GeneratedOfflineModelEntry.localizedDisplayName(language: String): String =
    displayName[language] ?: displayName["en"] ?: displayName.values.first()

/** Download lifecycle for one model (or an aggregate pack). */
sealed interface ModelState {
    data object NotInstalled : ModelState
    data class Queued(val bytes: Long, val total: Long) : ModelState
    data class Downloading(val bytes: Long, val total: Long) : ModelState
    data object Verifying : ModelState
    data object Extracting : ModelState
    data object Ready : ModelState
    data class Failed(val message: String) : ModelState
}

internal data class VoicePackProgress(
    val state: ModelState,
    val downloadedBytes: Long,
    val totalBytes: Long,
    val activeModel: GeneratedOfflineModelEntry?,
    val activeModelIndex: Int,
    val nextModel: GeneratedOfflineModelEntry?,
)

/** Combine the STT + TTS states into the progress shown for one language pack. */
internal fun aggregatePackState(
    states: Map<String, ModelState>,
    pack: GeneratedVoicePack,
): ModelState = voicePackProgress(states, pack).state

internal fun voicePackProgress(
    states: Map<String, ModelState>,
    pack: GeneratedVoicePack,
): VoicePackProgress {
    val modelStates = pack.models.map { states[it.id] ?: ModelState.NotInstalled }
    val total = pack.models.sumOf { model ->
        when (val state = states[model.id]) {
            is ModelState.Queued -> state.total.coerceAtLeast(model.approxSizeBytes)
            is ModelState.Downloading -> state.total.coerceAtLeast(model.approxSizeBytes)
            else -> model.approxSizeBytes
        }
    }
    val bytes = pack.models.sumOf { model ->
        when (val state = states[model.id]) {
            is ModelState.Queued -> state.bytes
            is ModelState.Downloading -> state.bytes
            is ModelState.Verifying, is ModelState.Extracting, is ModelState.Ready -> model.approxSizeBytes
            else -> 0L
        }
    }
    val activeIndex = modelStates.indexOfFirst { state ->
        state is ModelState.Downloading ||
            state is ModelState.Verifying ||
            state is ModelState.Extracting ||
            state is ModelState.Failed
    }.takeIf { it >= 0 } ?: modelStates.indexOfFirst { it is ModelState.Queued }
    val activeState = modelStates.getOrNull(activeIndex)
    val aggregateState = when {
        modelStates.all { it is ModelState.Ready } -> ModelState.Ready
        activeState is ModelState.Failed -> activeState
        activeState is ModelState.Verifying -> ModelState.Verifying
        activeState is ModelState.Extracting -> ModelState.Extracting
        activeState is ModelState.Downloading -> ModelState.Downloading(bytes, total.coerceAtLeast(bytes))
        activeState is ModelState.Queued -> ModelState.Queued(bytes, total.coerceAtLeast(bytes))
        else -> ModelState.NotInstalled
    }
    val nextIndex = if (activeIndex >= 0) {
        (activeIndex + 1 until pack.models.size).firstOrNull { modelStates[it] !is ModelState.Ready }
    } else {
        null
    }
    return VoicePackProgress(
        state = aggregateState,
        downloadedBytes = bytes,
        totalBytes = total.coerceAtLeast(bytes),
        activeModel = pack.models.getOrNull(activeIndex),
        activeModelIndex = activeIndex,
        nextModel = nextIndex?.let(pack.models::get),
    )
}
