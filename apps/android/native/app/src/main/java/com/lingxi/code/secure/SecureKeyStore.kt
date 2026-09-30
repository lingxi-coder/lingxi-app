package com.lingxi.code.secure

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/**
 * Encrypted-at-rest store for the engine's LLM credentials — the Anthropic API
 * key and an optional base URL (SHIP-BLOCKER #1).
 *
 * A shipped mobile app has no process environment to read `ANTHROPIC_API_KEY`
 * from, so the key must be configurable AND stored securely on-device. This
 * wraps [EncryptedSharedPreferences]: values are encrypted with AES-256-GCM and
 * the data key is wrapped by a [MasterKey] held in the hardware-backed Android
 * Keystore — so the on-disk prefs file is ciphertext, never plaintext (unlike a
 * plain DataStore / SharedPreferences).
 *
 * The engine reads the key from here FIRST (see
 * `EngineConversationSource.create`), falling back to `System.getenv` only as a
 * dev override. The masked Settings field (LLM provider page) reads/writes it.
 *
 * Construction can fail on devices with a broken Keystore (rare); callers build
 * via [create], which returns `null` on failure so the engine degrades to the
 * env / mock path rather than crashing the shell.
 */
class SecureKeyStore private constructor(private val prefs: SharedPreferences) {

    /** The stored Anthropic API key, or `""` when none has been set. */
    fun apiKey(): String = prefs.getString(KEY_API_KEY, null).orEmpty()

    /** The stored base URL override, or `""` when none has been set. */
    fun apiBase(): String = prefs.getString(KEY_API_BASE, null).orEmpty()

    /**
     * Persist the API key. A blank value clears the entry (so "delete the key"
     * is just "set it to empty") — the field never stores a stray empty string
     * that would shadow the env fallback with a non-null-but-blank value.
     */
    fun setApiKey(value: String) = edit { p ->
        if (value.isBlank()) p.remove(KEY_API_KEY) else p.putString(KEY_API_KEY, value.trim())
    }

    /** Persist the base URL override; a blank value clears the entry. */
    fun setApiBase(value: String) = edit { p ->
        if (value.isBlank()) p.remove(KEY_API_BASE) else p.putString(KEY_API_BASE, value.trim())
    }

    /** Wipe both stored credentials. */
    fun clear() = edit { it.clear() }

    private inline fun edit(block: (SharedPreferences.Editor) -> Unit) {
        prefs.edit().also(block).apply()
    }

    companion object {
        private const val FILE_NAME = "engine_secrets"
        private const val KEY_API_KEY = "anthropic_api_key"
        private const val KEY_API_BASE = "anthropic_base_url"

        /**
         * Build the encrypted store, or `null` if the platform Keystore can't be
         * provisioned (degrade to env / mock rather than crash). The [MasterKey]
         * is created once and reused across launches; the prefs file is the
         * ciphertext container.
         */
        fun create(context: Context): SecureKeyStore? = try {
            val appContext = context.applicationContext
            val masterKey = MasterKey.Builder(appContext)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
            val prefs = EncryptedSharedPreferences.create(
                appContext,
                FILE_NAME,
                masterKey,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            )
            SecureKeyStore(prefs)
        } catch (t: Throwable) {
            null
        }
    }
}

/**
 * The resolved LLM credentials handed to the engine builder — the key, base URL
 * and model that `buildVoiceEngine` receives.
 */
data class EngineCredentials(
    val apiKey: String,
    val apiBase: String,
    val model: String,
)

/**
 * PURE credential-resolution precedence — extracted as a free function with NO
 * Android dependency so it is exhaustively unit-testable on the JVM (where the
 * Keystore-backed [SecureKeyStore] is unavailable).
 *
 * Precedence is **secure store FIRST, environment as a dev override fallback**,
 * applied independently per field:
 *  - `apiKey`: a non-blank stored key wins; otherwise the env `ANTHROPIC_API_KEY`.
 *  - `apiBase`: a non-blank stored base wins; otherwise the env `ANTHROPIC_BASE_URL`.
 *  - `model`: env `LINGXI_MODEL` (the model isn't a secret — not stored).
 *
 * A blank everywhere `apiKey` is valid and returned as `""` — the caller keeps
 * the existing mock fallback for an empty key, and a configured turn 401s at run
 * time (matching the prior env-only behavior).
 */
fun resolveEngineCredentials(
    storedKey: String,
    storedBase: String,
    env: Map<String, String>,
): EngineCredentials = EngineCredentials(
    apiKey = storedKey.ifBlank { env["ANTHROPIC_API_KEY"].orEmpty() },
    apiBase = storedBase.ifBlank { env["ANTHROPIC_BASE_URL"].orEmpty() },
    model = env["LINGXI_MODEL"].orEmpty(),
)
