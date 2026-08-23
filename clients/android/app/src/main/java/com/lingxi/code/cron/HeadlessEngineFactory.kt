package com.lingxi.code.cron

import android.content.Context
import android.util.Log
import com.lingxi.code.bindings.AndroidLaunchModeFfi
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.secure.resolveEngineCredentials
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.voice.buildVoiceEngine

/**
 * Builds a NO-UI [MobileEngineHandle] for the cron background path — the same
 * engine the foreground app builds (same `filesDir` root ⇒ the same
 * `.lingxi/scheduled_tasks.json`, the same `SecureKeyStore` credentials), but
 * with discarding event / permission sinks because a fired cron job runs
 * headless: its result comes back from the single-task cron FFI, not from
 * streamed events. Rust immediately denies any permission that was not already
 * allowed by persistent policy; a background job never waits for UI approval.
 * Direct builds intentionally expose the Computer Use host to this engine, but
 * the host accepts calls only while the user has an active control session with
 * in-memory app grants. Those grants are never persisted or recreated by a
 * Worker, so stopping, locking, expiry, or process death closes background
 * access immediately.
 *
 * Returns `null` only when the engine is unavailable. Provider credentials stay
 * inside the shared encrypted Rust credential store; no Kotlin preflight reads
 * or exposes them. The caller MUST release the handle (`destroy()`) when done —
 * these are short-lived, per-run instances.
 */
object HeadlessEngineFactory {

    private const val TAG = "CronHeadlessEngine"

    /** Build a transient headless engine, or `null` if unavailable. */
    fun build(context: Context, scope: CronScope = CronScope.global(context)): MobileEngineHandle? {
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
            visionDelegationEnabled = providerLaunch.visionDelegationEnabled,
            projectWorkspace = scope.projectId?.let {
                ProjectWorkspace(
                    projectId = it,
                    hostPath = scope.workspacePath,
                    guestPath = scope.guestPath,
                )
            },
            launchMode = AndroidLaunchModeFfi.SCHEDULED_HEADLESS,
            // Discard streamed turn events — the cron result is returned by the
            // single-task cron FFI, not by the event stream.
            onEvent = { },
            // Headless: no interactive answerer. Rust rejects any permission
            // that was not already granted by the persisted policy.
            onPermission = { },
        ).also {
            if (it == null) Log.w(TAG, "headless mobile engine is unavailable")
        }
    }
}
