package com.lingxi.code.teams

import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.Future
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.ThreadFactory
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Cancellation closes the socket, including while a blocking read is in flight.
 * A deadline and bounded executor also bound transports that ignore interruption. */
internal object TeamHttp {
    private val threads = ThreadFactory { action -> Thread(action, "teams-http").apply { isDaemon = true } }
    private val requests = ThreadPoolExecutor(4, 4, 0, TimeUnit.SECONDS, ArrayBlockingQueue(32), threads)
    private val closing = ThreadPoolExecutor(4, 4, 0, TimeUnit.SECONDS, ArrayBlockingQueue(32), threads)
    private val deadlines = ScheduledThreadPoolExecutor(1, threads).apply { removeOnCancelPolicy = true }

    suspend fun request(url: String, token: String, body: JSONObject?): JSONObject = suspendCancellableCoroutine { continuation ->
        val finished = AtomicBoolean(false)
        val connection = AtomicReference<HttpURLConnection?>()
        val job = AtomicReference<Future<*>?>()
        fun cancelTransport() {
            job.get()?.let { it.cancel(true); requests.remove(it as Runnable) }
            connection.get()?.let { socket ->
                // disconnect can wait for a platform socket lock; keep it off Main.
                try { closing.execute { socket.disconnect() } } catch (_: java.util.concurrent.RejectedExecutionException) { /* The request's finally and read deadline still close it. */ }
            }
        }
        val deadline = deadlines.schedule({
            if (finished.compareAndSet(false, true)) {
                continuation.resumeWithException(TeamMessageFailure("SERVICE_UNAVAILABLE"))
                cancelTransport()
            }
        }, 20, TimeUnit.SECONDS)
        continuation.invokeOnCancellation { finished.set(true); deadline.cancel(false); cancelTransport() }
        try {
            val task = requests.submit {
                var socket: HttpURLConnection? = null
                try {
                    if (finished.get()) return@submit
                    socket = URL(url).openConnection() as HttpURLConnection
                    connection.set(socket)
                    if (finished.get()) return@submit
                    socket.connectTimeout = 20000; socket.readTimeout = 20000; socket.instanceFollowRedirects = false; socket.useCaches = false
                    socket.setRequestProperty("Authorization", "Bearer $token")
                    if (body != null) {
                        socket.requestMethod = "POST"; socket.doOutput = true; socket.setRequestProperty("Content-Type", "application/json")
                        socket.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
                    }
                    val success = socket.responseCode in 200..299
                    val stream = if (success) socket.inputStream else socket.errorStream
                    val value = JSONObject(stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: "{}")
                    if (!success) throw TeamMessageFailure(value.optJSONObject("error")?.optString("code") ?: "SERVICE_UNAVAILABLE")
                    if (finished.compareAndSet(false, true)) continuation.resume(value)
                } catch (error: Exception) {
                    if (finished.compareAndSet(false, true)) continuation.resumeWithException(error)
                } finally { deadline.cancel(false); socket?.disconnect() }
            }
            job.set(task)
            if (finished.get()) cancelTransport()
        } catch (error: java.util.concurrent.RejectedExecutionException) {
            deadline.cancel(false)
            if (finished.compareAndSet(false, true)) continuation.resumeWithException(TeamMessageFailure("SERVICE_UNAVAILABLE"))
        }
    }
}
