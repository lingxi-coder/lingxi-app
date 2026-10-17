package com.lingxi.code.push

import android.app.NotificationManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.SystemClock
import android.util.AtomicFile
import androidx.core.app.NotificationManagerCompat
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.WorkManager
import com.google.firebase.FirebaseApp
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseUser
import com.google.firebase.messaging.FirebaseMessaging
import com.lingxi.code.BuildConfig
import com.lingxi.code.settings.NotificationPrefsStore
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Opt-in production-device check, coordinated by verify-push-accounts.mjs.
 * Uses real Firebase Auth, FCM and WorkManager. The coordinator backs up and
 * restores the opaque encrypted Auth preferences after this process exits.
 */
@RunWith(AndroidJUnit4::class)
class PushAccountDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val directory = File(context.noBackupFilesDir, "push-live-verification")
    private val status = AtomicFile(File(directory, "status.json"))
    private val control = File(directory, "control.json")
    private lateinit var session: String
    private var phase = "starting"
    private var originalUid: String? = null
    private var temporaryUid: String? = null
    private var temporaryDeleted = false
    private var lastProbeAt = 0L
    private var lastProbeResult = false

    /** Operator-selected baseline repair, never part of the offline assertion. */
    @Test
    fun refreshFcmRegistration() = runBlocking {
        assumeTrue(InstrumentationRegistry.getArguments().getString("livePushProject") == "lingxi-5e4b2")
        check(BuildConfig.DEBUG && FirebaseApp.getInstance().options.projectId == "lingxi-5e4b2")
        withTimeout(120_000) {
            FirebaseMessaging.getInstance().unregister().await()
            FirebaseMessaging.getInstance().register().await()
            PushRegistration.enqueue(context)
        }
    }

    @Test
    fun accountSwitchAndOfflineLogout() = runBlocking {
        val args = InstrumentationRegistry.getArguments()
        assumeTrue("Live production check requires explicit opt-in", args.getString("livePushProject") == "lingxi-5e4b2")
        check(BuildConfig.DEBUG && FirebaseApp.getInstance().options.projectId == "lingxi-5e4b2")
        session = UUID.fromString(checkNotNull(args.getString("livePushSession"))).toString()
        directory.mkdirs()
        val auth = FirebaseAuth.getInstance()
        originalUid = checkNotNull(auth.currentUser) { "Sign in before running the live device check" }.uid
        check(NotificationPrefsStore(context).load().enabled && NotificationManagerCompat.from(context).areNotificationsEnabled())
        var temporary: FirebaseUser? = null
        var failure: Throwable? = null
        try {
            withTimeout(12 * 60_000L) {
                PushRegistration.enqueue(context)
                phase = "account_a"
                awaitCommand("switch")

                val priorRevision = PushRegistration.identity(context).revision
                temporary = auth.createUserWithEmailAndPassword(
                    "fcm-verify-${UUID.randomUUID()}@example.com", "Aa!${UUID.randomUUID()}",
                ).await().user
                temporaryUid = checkNotNull(temporary).uid
                check(temporaryUid != originalUid)
                instrumentation.waitForIdleSync()
                withTimeout(15_000) {
                    while (PushRegistration.identity(context).revision <= priorRevision) delay(100)
                }
                phase = "account_b"
                awaitCommand("logout_offline")

                check(!backendReachable(force = true)) { "Device must be offline before sign-out" }
                val boundRevision = PushRegistration.identity(context).revision
                auth.signOut()
                instrumentation.waitForIdleSync()
                withTimeout(15_000) {
                    while (PushRegistration.identity(context).revision <= boundRevision) delay(100)
                }
                check(auth.currentUser == null && PushRegistration.identity(context).mayBeBound)
                phase = "offline_logout"
                awaitCommand("cleanup")
                check(backendReachable(force = true) && auth.currentUser == null)
                check(!PushRegistration.identity(context).mayBeBound) { "Logout must finish before restoring the account" }

                // Delete only the account this invocation created. The original
                // user's credentials, profile and permissions are never changed.
                check(temporary.uid == temporaryUid && temporaryUid != originalUid)
                checkNotNull(temporary).delete().await()
                temporaryDeleted = true
                instrumentation.waitForIdleSync()
                check(auth.currentUser == null)
                phase = "logged_out"
                awaitCommand("finish")
            }
        } catch (error: Throwable) {
            failure = error
        } finally {
            // The coordinator restores network settings even if an assertion
            // fails. Keep waiting briefly so temporary-account cleanup can finish.
            withContext(NonCancellable) {
                withTimeout(90_000) {
                    while (!backendReachable(force = true)) delay(500)
                }
                try {
                    if (!temporaryDeleted && temporary != null) {
                        val cleanupUser = checkNotNull(temporary)
                        check(cleanupUser.uid != originalUid)
                        cleanupUser.delete().await()
                        temporaryDeleted = true
                    }
                } finally {
                    instrumentation.waitForIdleSync()
                    phase = if (failure == null) "complete" else "failed"
                    publish(failure?.javaClass?.simpleName)
                }
            }
        }
        failure?.let { throw AssertionError("Live push check failed: ${it.javaClass.simpleName}") }
        Unit
    }

    private fun validatedNetwork(): Boolean {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        return connectivity.getNetworkCapabilities(connectivity.activeNetwork)
            ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true
    }

    private fun backendReachable(force: Boolean = false): Boolean {
        if (!force && SystemClock.elapsedRealtime() - lastProbeAt < 2_000) return lastProbeResult
        val connection = URL("https://us-central1-lingxi-5e4b2.cloudfunctions.net/pushApi/health")
            .openConnection() as HttpURLConnection
        lastProbeResult = try {
            connection.connectTimeout = 3_000
            connection.readTimeout = 3_000
            connection.useCaches = false
            connection.instanceFollowRedirects = false
            connection.responseCode == 200
        } catch (_: Exception) { false }
        finally { connection.disconnect() }
        lastProbeAt = SystemClock.elapsedRealtime()
        return lastProbeResult
    }

    private suspend fun awaitCommand(action: String) {
        withTimeout(180_000) {
            while (true) {
                publish()
                val command = runCatching { JSONObject(control.readText()) }.getOrNull()
                if (command?.optString("session") == session) {
                    check(command.optString("action") != "abort") { "Coordinator aborted live check" }
                    if (command.optString("action") == action) return@withTimeout
                }
                delay(400)
            }
        }
    }

    private fun publish(failure: String? = null) {
        val identity = PushRegistration.identity(context)
        val notifications = context.getSystemService(NotificationManager::class.java).activeNotifications
            .filter { it.tag == "lingxi_push" && it.notification.extras.getCharSequence("android.title")?.toString().orEmpty().startsWith("FCM QA ") }
            .map { JSONObject().put("title", it.notification.extras.getCharSequence("android.title").toString()) }
        val work = WorkManager.getInstance(context).getWorkInfosForUniqueWork("lingxi-push-registration")
            .get(5, TimeUnit.SECONDS).map { it.state.name }
        val value = JSONObject().put("session", session).put("phase", phase)
            .put("originalUid", originalUid).put("temporaryUid", temporaryUid)
            .put("uid", FirebaseAuth.getInstance().currentUser?.uid ?: JSONObject.NULL)
            .put("installationId", identity.id).put("revision", identity.revision)
            .put("mayBeBound", identity.mayBeBound).put("validatedNetwork", validatedNetwork())
            .put("backendReachable", backendReachable())
            .put("temporaryDeleted", temporaryDeleted).put("notifications", JSONArray(notifications))
            .put("work", JSONArray(work)).put("failureType", failure ?: JSONObject.NULL)
        val stream = status.startWrite()
        try { stream.write(value.toString().toByteArray()); status.finishWrite(stream) }
        catch (error: Throwable) { status.failWrite(stream); throw error }
    }
}
