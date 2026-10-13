package com.lingxi.code.voice

import com.lingxi.code.R
import java.util.Locale

/** Recovery copy is bounded and never exposes provider errors or credentials. */
enum class AudioSetupGuidance(val titleRes: Int, val detailRes: Int) {
    Permission(R.string.audio_guide_permission_title, R.string.audio_guide_permission_detail),
    Quota(R.string.audio_guide_quota_title, R.string.audio_guide_quota_detail),
    Configuration(R.string.audio_guide_configuration_title, R.string.audio_guide_configuration_detail),
    Retry(R.string.audio_guide_retry_title, R.string.audio_guide_retry_detail);

    companion object {
        fun fromError(kind: String = "", message: String = ""): AudioSetupGuidance? {
            val error = "$kind $message".lowercase(Locale.ROOT)
            if (error.contains("cancelled") || error.contains("canceled")) return null
            return when {
                error.contains("permission") || error.contains("denied") || error.contains("权限") || error.contains("權限") -> Permission
                error.contains("429") || error.contains("quota") -> Quota
                listOf("unsupported", "not supported", "unavailable", "model", "configuration", "language", "systemunavailable", "模型", "配置", "语言", "語言")
                    .any(error::contains) -> Configuration
                else -> Retry
            }
        }
    }
}
