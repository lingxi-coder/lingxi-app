package com.lingxi.code.push

import android.app.Application
import android.content.Context
import android.os.Build
import android.util.AtomicFile
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.installations.FirebaseInstallations
import com.lingxi.code.BuildConfig
import com.lingxi.code.settings.NotificationPrefsStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.SecureRandom
import java.util.Locale
import java.util.UUID
import java.util.concurrent.TimeUnit

class LingxiApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        PushRegistration.initialize(this)
    }
}

/** Installation scoped, excluded from Android Auto Backup (no hardware IDs). */
internal data class PushIdentity(
    val id: String, val secret: String, val revision: Long = 0, val mayBeBound: Boolean = false,
    val registrationId: String? = null,
)

object PushRegistration {
    private const val WORK = "lingxi-push-registration"
    private val network = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
    private val lock = Any()
    private var listener: FirebaseAuth.AuthStateListener? = null

    fun initialize(context: Context) {
        if (listener != null) return
        listener = FirebaseAuth.AuthStateListener {
            PushNotifications.clear(context)
            enqueue(context)
        }.also { FirebaseAuth.getInstance().addAuthStateListener(it) }
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            "$WORK-heartbeat", ExistingPeriodicWorkPolicy.KEEP,
            PeriodicWorkRequestBuilder<PushRegistrationWorker>(7, TimeUnit.DAYS)
                .setConstraints(network).build(),
        )
    }

    fun enqueue(context: Context) {
        try {
            synchronized(lock) {
                val identity = read(context).let { it.copy(revision = it.revision + 1) }
                write(context, identity)
                WorkManager.getInstance(context).enqueueUniqueWork(
                    WORK, ExistingWorkPolicy.REPLACE,
                    OneTimeWorkRequestBuilder<PushRegistrationWorker>()
                        .setInputData(workDataOf("revision" to identity.revision))
                        .setConstraints(network).setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS).build(),
                )
            }
        } catch (_: Exception) {
            // FCM maintenance must not crash login/settings if private storage
            // is unavailable. Never log a JSON parse error containing secrets.
            Log.w("LingxiPush", "Unable to persist or schedule push registration")
        }
    }

    internal fun identity(context: Context): PushIdentity = synchronized(lock) { read(context) }

    fun registered(context: Context, fid: String) {
        try {
            synchronized(lock) {
                val current = read(context)
                // Manual register() also fires onRegistered. Avoid a register/upload
                // feedback loop for the same FID; foreground/weekly work renews leases.
                if (current.registrationId != fid) {
                    write(context, current.copy(registrationId = fid))
                    enqueue(context)
                }
            }
        } catch (_: Exception) {
            Log.w("LingxiPush", "Unable to persist FCM registration update")
        }
    }

    internal fun markBound(context: Context, revision: Long, bound: Boolean): Boolean = synchronized(lock) {
        val current = read(context)
        if (current.revision != revision) return@synchronized false
        write(context, current.copy(mayBeBound = bound))
        true
    }

    private fun file(context: Context) = AtomicFile(File(context.noBackupFilesDir, "push-installation.json"))
    private fun read(context: Context): PushIdentity {
        val file = file(context)
        if (!file.baseFile.exists()) {
            val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
            val identity = PushIdentity(UUID.randomUUID().toString(), bytes.joinToString("") { "%02x".format(it) })
            write(context, identity)
            return identity
        }
        // Do not silently reset a corrupt identity: that would orphan a binding.
        val json = JSONObject(file.openRead().bufferedReader().use { it.readText() })
        return PushIdentity(json.getString("id"), json.getString("secret"), json.getLong("revision"), json.getBoolean("mayBeBound"),
            json.optString("registrationId").takeIf(String::isNotEmpty))
    }
    private fun write(context: Context, value: PushIdentity) {
        val file = file(context)
        val stream = file.startWrite()
        try {
            stream.write(JSONObject().put("id", value.id).put("secret", value.secret)
                .put("revision", value.revision).put("mayBeBound", value.mayBeBound)
                .put("registrationId", value.registrationId).toString().toByteArray())
            file.finishWrite(stream)
        } catch (error: Exception) { file.failWrite(stream); throw error }
    }
}

class PushRegistrationWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val requestedRevision = inputData.getLong("revision", -1)
        if (requestedRevision == -1L) {
            PushRegistration.enqueue(applicationContext)
            return Result.success()
        }
        return try {
            val identity = PushRegistration.identity(applicationContext)
            if (identity.revision != requestedRevision) return Result.success()
            val auth = FirebaseAuth.getInstance()
            val user = auth.currentUser
            if (user == null && !identity.mayBeBound) return Result.success()
            val body = JSONObject().put("installationSecret", identity.secret).put("revision", identity.revision)
            val bearer = if (user != null) {
                FirebaseMessaging.getInstance().register().await()
                val fid = FirebaseInstallations.getInstance().id.await()
                val idToken = checkNotNull(user.getIdToken(runAttemptCount > 0).await().token)
                val initialized = request(identity.id, "POST", JSONObject().put("installationSecret", identity.secret), idToken, "/initialize")
                if (initialized !in 200..299) return if (initialized == 401 || initialized == 429 || initialized >= 500) Result.retry() else Result.failure()
                if (auth.currentUser?.uid != user.uid || !PushRegistration.markBound(applicationContext, identity.revision, true)) {
                    return Result.success()
                }
                body.put("fcmInstallationId", fid).put("platform", "android").put("appId", BuildConfig.APPLICATION_ID)
                    .put("appVersion", BuildConfig.VERSION_NAME).put("osVersion", Build.VERSION.RELEASE)
                    .put("deviceModel", "${Build.MANUFACTURER} ${Build.MODEL}").put("locale", Locale.getDefault().toLanguageTag())
                    .put("notificationsEnabled", NotificationPrefsStore(applicationContext).load().enabled &&
                        NotificationManagerCompat.from(applicationContext).areNotificationsEnabled())
                idToken
            } else null
            if (PushRegistration.identity(applicationContext).revision != identity.revision) return Result.success()
            val status = request(identity.id, if (user == null) "DELETE" else "PUT", body, bearer)
            when {
                status in 200..299 -> {
                    if (user == null) PushRegistration.markBound(applicationContext, identity.revision, false)
                    Result.success()
                }
                status == 401 || status == 429 || status >= 500 -> Result.retry()
                else -> Result.failure()
            }
        } catch (error: CancellationException) { throw error }
        catch (_: Exception) { Result.retry() }
    }

    private suspend fun request(id: String, method: String, body: JSONObject, bearer: String?, suffix: String = ""): Int = withContext(Dispatchers.IO) {
        val connection = URL("https://us-central1-lingxi-5e4b2.cloudfunctions.net/pushApi/v1/push/installations/$id$suffix")
            .openConnection() as HttpURLConnection
        try {
            connection.requestMethod = method
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15_000
            connection.readTimeout = 15_000
            connection.setRequestProperty("Content-Type", "application/json")
            bearer?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
            connection.doOutput = true
            connection.outputStream.use { it.write(body.toString().toByteArray()) }
            connection.responseCode
        } finally { connection.disconnect() }
    }
}
