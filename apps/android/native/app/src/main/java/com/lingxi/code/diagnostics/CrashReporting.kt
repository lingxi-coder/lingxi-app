package com.lingxi.code.diagnostics

import com.google.firebase.crashlytics.FirebaseCrashlytics
import com.lingxi.code.BuildConfig

/** Anonymous crash diagnostics. Do not attach account IDs, prompts or credentials. */
object CrashReporting {
    fun initialize() {
        FirebaseCrashlytics.getInstance().apply {
            setCustomKey("distribution", BuildConfig.DISTRIBUTION_CHANNEL)
            setCustomKey("build_type", BuildConfig.BUILD_TYPE)
        }
    }
}
