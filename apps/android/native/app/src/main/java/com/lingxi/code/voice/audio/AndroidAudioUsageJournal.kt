package com.lingxi.code.voice.audio

import org.json.JSONObject
import java.util.concurrent.ConcurrentLinkedDeque

internal data class AndroidAudioUsageRecord(
    val operationId: String,
    val kind: String,
    val profileId: String?,
    val accountScope: String?,
    val modelId: String?,
    val usageJson: String,
)

/** SDK usage is retained independently of native result DTOs; absent cost stays absent. */
internal object AndroidAudioUsageJournal {
    private val records = ConcurrentLinkedDeque<AndroidAudioUsageRecord>()
    fun record(operationId: String, kind: String, response: JSONObject) {
        val context = response.optJSONObject("usageContext")
        records.addLast(AndroidAudioUsageRecord(operationId, kind,
            context?.optionalUsageString("profileId"), context?.optionalUsageString("accountScope"),
            context?.optionalUsageString("modelId"), response.opt("usage")?.toString() ?: "null"))
        while (records.size > 128) records.pollFirst()
    }
    fun snapshot(): List<AndroidAudioUsageRecord> = records.toList()
}

private fun JSONObject.optionalUsageString(key: String): String? = if (isNull(key)) null else optString(key).takeIf(String::isNotBlank)
