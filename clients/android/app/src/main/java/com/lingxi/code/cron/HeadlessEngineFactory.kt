package com.lingxi.code.cron

import android.content.Context
import android.util.Log
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.secure.resolveEngineCredentials
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.voice.buildVoiceEngine

/**
 * Builds a NO-UI [MobileEngineHandle] for the cron background path — the same
 * engine the foreground app builds (same `filesDir` root ⇒ the same
 * `.lingxi/scheduled_tasks.json`, the same `SecureKeyStore` credentials), but
 * with discarding event / permission sinks because a fired cron job runs
 * headless: its result comes back from `runDueCronNow()`, not from streamed
 * events, and there is no human to answer a permission prompt (the Rust gate
 * enforces the inherited session policy — claude-code parity).
 *
 * Returns `null` only when the engine is unavailable. Provider credentials stay
 * inside the shared encrypted Rust credential store; no Kotlin preflight reads
 * or exposes them. The caller MUST release the handle (`destroy()`) when done —
 * these are short-lived, per-run instances.
 */
object HeadlessEngineFactory {

    private const val TAG = "CronHeadlessEngine"

    /** Build a transient headless engine, or `null` if unavailable. */
    fun build(context: Context): MobileEngineHandle? {
        val appContext = context.applicationContext
        val store = SecureKeyStore.create(appContext)
        val creds = resolveEngineCredentials(
            storedKey = store?.apiKey() ?: "",
            storedBase = store?.apiBase() ?: "",
            env = System.getenv(),
        )
        val providerLaunch = ProviderSettingsRepository(appContext).engineLaunchConfig()
        return buildVoiceEngine(
            context = appContext,
            apiBase = creds.apiBase,
            apiKey = creds.apiKey,
            model = creds.model.ifBlank { providerLaunch.defaultModel },
            providerProfilesJson = providerLaunch.providerProfilesJson,
            routingJson = providerLaunch.routingJson,
            // Discard streamed turn events — the cron result is the
            // `runDueCronNow()` return value, not the event stream.
            onEvent = { },
            // Headless: no interactive answerer. The Rust `PolicyPermissionGate`
            // still enforces the session's allow/deny/defaultMode policy; a
            // prompting tool simply parks until the run's budget elapses.
            onPermission = { },
        ).also {
            if (it == null) Log.w(TAG, "headless mobile engine is unavailable")
        }
    }
}
