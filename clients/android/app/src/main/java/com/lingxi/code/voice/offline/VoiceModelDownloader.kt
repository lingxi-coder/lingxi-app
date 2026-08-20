package com.lingxi.code.voice.offline

import android.content.Context
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream
import java.io.BufferedInputStream
import java.io.EOFException
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.net.URL
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.coroutineContext

private const val CONNECT_TIMEOUT_MS = 45_000
private const val READ_TIMEOUT_MS = 120_000
private const val MAX_DOWNLOAD_ATTEMPTS = 4

private class DownloadHttpException(
    val statusCode: Int,
    val retryable: Boolean,
    message: String,
) : IOException(message)

private data class ContentRange(
    val start: Long?,
    val total: Long?,
)

private fun parseContentRange(value: String?): ContentRange? {
    if (value.isNullOrBlank()) return null
    Regex("""bytes\s+(\d+)-\d+/(\d+|\*)""", RegexOption.IGNORE_CASE)
        .matchEntire(value.trim())
        ?.let { match ->
            return ContentRange(
                start = match.groupValues[1].toLongOrNull(),
                total = match.groupValues[2].takeUnless { it == "*" }?.toLongOrNull(),
            )
        }
    Regex("""bytes\s+\*/(\d+)""", RegexOption.IGNORE_CASE)
        .matchEntire(value.trim())
        ?.let { match -> return ContentRange(start = null, total = match.groupValues[1].toLongOrNull()) }
    return null
}

private fun openDownloadConnection(url: URL): HttpURLConnection =
    (url.openConnection() as HttpURLConnection).apply {
        connectTimeout = CONNECT_TIMEOUT_MS
        readTimeout = READ_TIMEOUT_MS
        instanceFollowRedirects = true
        setRequestProperty("Accept-Encoding", "identity")
        setRequestProperty("User-Agent", "LingXi-Android/0.1")
    }

/**
 * Download [sourceUrl] into [destination], preserving partial bytes across
 * transient failures and process-local retries. Servers that ignore Range are
 * handled safely by truncating and restarting the file.
 *
 * [context] is optional (and stays `null` in [VoiceModelDownloaderTest], a
 * pure-JVM suite with no `Context` at all) so the failure messages this
 * throws stay the literal zh-Hans copy when absent, and resolve to the real
 * localized text when the production caller ([VoiceModelDownloader.runDownload])
 * passes its `appContext`.
 */
internal suspend fun downloadArchiveWithResume(
    sourceUrl: String,
    destination: File,
    fallbackTotal: Long,
    maxAttempts: Int = MAX_DOWNLOAD_ATTEMPTS,
    connectionFactory: (URL) -> HttpURLConnection = ::openDownloadConnection,
    retryDelay: suspend (attempt: Int) -> Unit = { attempt ->
        delay((1_000L shl attempt.coerceAtMost(3)))
    },
    context: Context? = null,
    onProgress: (bytes: Long, total: Long) -> Unit,
) {
    require(maxAttempts > 0)
    destination.parentFile?.mkdirs()
    var lastFailure: IOException? = null

    repeat(maxAttempts) { attempt ->
        coroutineContext.ensureActive()
        try {
            downloadAttempt(
                sourceUrl = sourceUrl,
                destination = destination,
                fallbackTotal = fallbackTotal,
                connectionFactory = connectionFactory,
                context = context,
                onProgress = onProgress,
            )
            return
        } catch (error: DownloadHttpException) {
            if (!error.retryable) throw error
            lastFailure = error
        } catch (error: IOException) {
            lastFailure = error
        }

        if (attempt < maxAttempts - 1) retryDelay(attempt)
    }

    // Unreachable: every failed attempt above sets `lastFailure` (the only
    // path that doesn't is a non-retryable DownloadHttpException, which is
    // re-thrown immediately, never falling through to here). Kept as a
    // defensive fallback for the compiler's non-null `throw`.
    throw lastFailure ?: IOException("下载失败")
}

private suspend fun downloadAttempt(
    sourceUrl: String,
    destination: File,
    fallbackTotal: Long,
    connectionFactory: (URL) -> HttpURLConnection,
    context: Context?,
    onProgress: (bytes: Long, total: Long) -> Unit,
) {
    var existingBytes = destination.takeIf { it.isFile }?.length() ?: 0L
    val connection = connectionFactory(URL(sourceUrl))
    try {
        if (existingBytes > 0L) {
            connection.setRequestProperty("Range", "bytes=$existingBytes-")
        }
        connection.connect()

        val status = connection.responseCode
        val contentRange = parseContentRange(connection.getHeaderField("Content-Range"))
        if (status == 416) {
            if (contentRange?.total == existingBytes) {
                onProgress(existingBytes, existingBytes)
                return
            }
            destination.delete()
            throw DownloadHttpException(
                status,
                retryable = true,
                message = context?.getString(R.string.voice_download_resume_rejected) ?: "服务器拒绝续传",
            )
        }
        if (status !in 200..299) {
            val retryable = status == 408 || status == 429 || status in 500..599
            throw DownloadHttpException(
                status,
                retryable,
                context?.getString(R.string.voice_download_http_status_fmt, status)
                    ?: "下载服务器返回 HTTP $status",
            )
        }

        val append = status == HttpURLConnection.HTTP_PARTIAL && existingBytes > 0L
        if (status == HttpURLConnection.HTTP_PARTIAL && contentRange?.start != existingBytes) {
            destination.delete()
            throw DownloadHttpException(
                status,
                retryable = true,
                message = context?.getString(R.string.voice_download_resume_mismatch) ?: "续传位置不匹配",
            )
        }
        if (!append) existingBytes = 0L

        val responseBytes = connection.contentLengthLong
        val totalBytes = (
            contentRange?.total
                ?: responseBytes.takeIf { it > 0L }?.plus(existingBytes)
                ?: fallbackTotal
        ).coerceAtLeast(existingBytes)
        var completedBytes = existingBytes
        onProgress(completedBytes, totalBytes)

        connection.inputStream.use { input ->
            FileOutputStream(destination, append).buffered().use { output ->
                val buffer = ByteArray(64 * 1024)
                while (true) {
                    coroutineContext.ensureActive()
                    val count = input.read(buffer)
                    if (count < 0) break
                    output.write(buffer, 0, count)
                    completedBytes += count
                    onProgress(completedBytes, totalBytes.coerceAtLeast(completedBytes))
                }
            }
        }

        if (responseBytes > 0L && completedBytes < existingBytes + responseBytes) {
            throw EOFException(context?.getString(R.string.voice_download_truncated) ?: "下载连接提前结束")
        }
    } finally {
        connection.disconnect()
    }
}

/**
 * Downloads sherpa-onnx voice models on demand (the setup wizard's language-pack
 * step). Self-contained port of the relevant slice of ~/lingxi/android's
 * OfflineModelManager: stream the `.tar.bz2` with progress → verify SHA-256 →
 * install the declared files into `filesDir/voice_models/<id>/`.
 *
 * Process-global (attached with the application Context in MainActivity, like the
 * Camera/Share/etc. controllers) so a download survives leaving the wizard step.
 * The native sherpa runtime loads the verified model directories directly.
 */
object VoiceModelDownloader {
    private var appContext: Context? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val jobs = ConcurrentHashMap<String, Job>()
    private val downloadSlot = Semaphore(1)

    private val _states = MutableStateFlow<Map<String, ModelState>>(emptyMap())
    val states: StateFlow<Map<String, ModelState>> = _states.asStateFlow()

    fun attach(context: Context) {
        appContext = context.applicationContext
        reconcileFromDisk()
    }

    fun modelDir(id: String): File = File(appContext!!.filesDir, "voice_models/$id")

    fun isReady(entry: OfflineModelEntry): Boolean {
        if (appContext == null) return false
        val dir = modelDir(entry.id)
        return entry.files.isNotEmpty() &&
            entry.files.all { File(dir, it).isFile } &&
            entry.requiredDirectories.all { File(dir, it).isDirectory }
    }

    /** Mark already-on-disk models Ready so re-entering the wizard reflects reality. */
    private fun reconcileFromDisk() {
        restoreInterruptedInstallations()
        removeObsoletePartials()
        _states.value = OfflineModelCatalog.all.associate {
            it.id to if (isReady(it)) ModelState.Ready else (_states.value[it.id] ?: ModelState.NotInstalled)
        }
    }

    private fun removeObsoletePartials() {
        val ctx = appContext ?: return
        val currentArchives = OfflineModelCatalog.all.mapTo(mutableSetOf()) { "${it.id}.tar.bz2" }
        File(ctx.cacheDir, "voice_dl").listFiles()
            ?.filter { it.isFile && it.name.endsWith(".tar.bz2") && it.name !in currentArchives }
            ?.forEach(File::delete)
    }

    private fun restoreInterruptedInstallations() {
        OfflineModelCatalog.all.forEach { entry ->
            val destination = modelDir(entry.id)
            val backup = File(destination.parentFile, ".${entry.id}.previous")
            val staging = File(destination.parentFile, ".${entry.id}.installing")
            when {
                destination.exists() -> backup.deleteRecursively()
                backup.exists() -> runCatching { moveDirectory(backup, destination) }
            }
            staging.deleteRecursively()
        }
    }

    /** Start (or resume) downloading every model in a language pack. */
    fun startPack(language: String) {
        val entries = OfflineModelCatalog.packFor(language)
        if (entries.isEmpty()) return
        if (entries.any { entry ->
                when (_states.value[entry.id]) {
                    is ModelState.Queued, is ModelState.Downloading,
                    is ModelState.Verifying, is ModelState.Extracting -> true
                    else -> false
                }
            }
        ) {
            return
        }

        entries.filter(::isReady).forEach { set(it.id, ModelState.Ready) }
        val pending = entries.filterNot(::isReady)
        if (pending.isEmpty()) {
            entries.forEach { set(it.id, ModelState.Ready) }
            return
        }
        pending.forEach(::markQueued)

        val job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                for (entry in pending) {
                    runQueuedDownload(entry)
                    if (_states.value[entry.id] is ModelState.Failed) break
                }
            } finally {
                pending.forEach { entry ->
                    coroutineContext[Job]?.let { currentJob -> jobs.remove(entry.id, currentJob) }
                    if (_states.value[entry.id] is ModelState.Queued) {
                        set(entry.id, ModelState.NotInstalled)
                    }
                }
            }
        }
        pending.forEach { entry -> jobs.put(entry.id, job)?.cancel() }
        job.start()
    }

    fun start(entry: OfflineModelEntry) {
        when (_states.value[entry.id]) {
            is ModelState.Ready, is ModelState.Queued, is ModelState.Downloading,
            is ModelState.Verifying, is ModelState.Extracting -> return
            else -> {}
        }
        if (appContext == null) {
            // No Context to localize with by definition (this branch only runs
            // when [attach] hasn't been called yet — never in practice, since
            // MainActivity.onCreate always attaches before any UI can reach
            // `start()`). Kept as the literal zh-Hans copy.
            set(entry.id, ModelState.Failed("下载器尚未初始化"))
            return
        }
        markQueued(entry)

        val job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                runQueuedDownload(entry)
            } finally {
                coroutineContext[Job]?.let { jobs.remove(entry.id, it) }
            }
        }
        jobs.put(entry.id, job)?.cancel()
        job.start()
    }

    fun cancel(entry: OfflineModelEntry) {
        jobs.remove(entry.id)?.cancel()
        set(entry.id, if (isReady(entry)) ModelState.Ready else ModelState.NotInstalled)
    }

    private fun set(id: String, state: ModelState) = _states.update { it + (id to state) }

    private fun markQueued(entry: OfflineModelEntry) {
        val ctx = appContext ?: return
        val partial = File(ctx.cacheDir, "voice_dl/${entry.id}.tar.bz2").takeIf { it.isFile }?.length() ?: 0L
        set(entry.id, ModelState.Queued(partial, entry.approxSizeBytes.coerceAtLeast(partial)))
    }

    private suspend fun runQueuedDownload(entry: OfflineModelEntry) {
        downloadSlot.withPermit {
            val ctx = appContext ?: return@withPermit
            val partial = File(ctx.cacheDir, "voice_dl/${entry.id}.tar.bz2").takeIf { it.isFile }?.length() ?: 0L
            set(entry.id, ModelState.Downloading(partial, entry.approxSizeBytes.coerceAtLeast(partial)))
            runDownload(entry)
        }
    }

    private suspend fun runDownload(entry: OfflineModelEntry) {
        val ctx = appContext ?: return
        val tmp = File(ctx.cacheDir, "voice_dl/${entry.id}.tar.bz2")
        try {
            tmp.parentFile?.mkdirs()
            downloadArchiveWithResume(
                sourceUrl = entry.sourceUrl,
                destination = tmp,
                fallbackTotal = entry.approxSizeBytes,
                context = ctx,
            ) { bytes, total ->
                set(entry.id, ModelState.Downloading(bytes, total))
            }
            set(entry.id, ModelState.Verifying)
            val hex = sha256(tmp)
            if (!hex.equals(entry.sha256, ignoreCase = true)) {
                tmp.delete()
                set(entry.id, ModelState.Failed(ctx.getString(R.string.voice_download_checksum_mismatch)))
                return
            }
            set(entry.id, ModelState.Extracting)
            install(tmp, entry, modelDir(entry.id))
            tmp.delete()
            set(
                entry.id,
                if (isReady(entry)) ModelState.Ready else ModelState.Failed(ctx.getString(R.string.voice_download_extracted_files_missing)),
            )
        } catch (ce: CancellationException) {
            set(entry.id, ModelState.NotInstalled)
            throw ce
        } catch (e: Throwable) {
            set(entry.id, ModelState.Failed(failureMessage(e, tmp.length())))
        }
    }

    private suspend fun sha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                coroutineContext.ensureActive()
                val count = input.read(buffer)
                if (count < 0) break
                digest.update(buffer, 0, count)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    private fun failureMessage(error: Throwable, partialBytes: Long): String {
        // `DownloadHttpException`/the `EOFException` thrown by `downloadAttempt`
        // above already carry pre-localized `.message` text (it was resolved
        // via `context?.getString` at the throw site, using this same
        // `appContext`) — only the exception TYPES this function can't
        // localize at their own throw site (JDK-thrown SocketTimeoutException /
        // UnknownHostException, plus the generic IOException/else fallback)
        // need to resolve their copy here.
        val ctx = appContext
        val resumable = if (partialBytes > 0L) {
            ctx?.getString(R.string.voice_download_resumable_suffix) ?: "，已保留进度"
        } else {
            ""
        }
        return when (error) {
            is SocketTimeoutException -> (ctx?.getString(R.string.voice_download_timeout) ?: "连接超时") + resumable
            is UnknownHostException -> (ctx?.getString(R.string.voice_download_dns_failed) ?: "无法解析下载地址") + resumable
            is DownloadHttpException -> "${error.message}$resumable"
            is IOException -> "${error.message ?: (ctx?.getString(R.string.voice_download_network_interrupted) ?: "网络中断")}$resumable"
            else -> error.message ?: (ctx?.getString(R.string.voice_download_failed) ?: "下载失败")
        }
    }

    private fun install(archive: File, entry: OfflineModelEntry, destDir: File) {
        val ctx = appContext
        val staging = File(destDir.parentFile, ".${entry.id}.installing")
        staging.deleteRecursively()
        extract(archive, entry, staging)
        if (
            !entry.files.all { File(staging, it).isFile } ||
            !entry.requiredDirectories.all { File(staging, it).isDirectory }
        ) {
            staging.deleteRecursively()
            throw IOException(ctx?.getString(R.string.voice_download_extracted_files_missing) ?: "解压后文件缺失")
        }
        val backup = File(destDir.parentFile, ".${entry.id}.previous")
        backup.deleteRecursively()
        var published = false
        try {
            if (destDir.exists()) moveDirectory(destDir, backup)
            moveDirectory(staging, destDir)
            published = true
            backup.deleteRecursively()
        } catch (error: Exception) {
            if (published && destDir.exists()) destDir.deleteRecursively()
            if (backup.exists()) runCatching { moveDirectory(backup, destDir) }
            staging.deleteRecursively()
            throw IOException(
                ctx?.getString(R.string.voice_download_activate_failed) ?: "无法激活语音模型",
                error,
            )
        }
    }

    private fun moveDirectory(source: File, destination: File) {
        try {
            Files.move(
                source.toPath(),
                destination.toPath(),
                StandardCopyOption.ATOMIC_MOVE,
                StandardCopyOption.REPLACE_EXISTING,
            )
        } catch (_: AtomicMoveNotSupportedException) {
            Files.move(
                source.toPath(),
                destination.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    /**
     * Extract only runtime files, required data directories, and bundled
     * attribution files. Large alternate-precision models and sample WAVs are
     * intentionally left out to avoid doubling the installed size.
     */
    private fun extract(archive: File, entry: OfflineModelEntry, destDir: File) {
        val ctx = appContext
        val root = destDir.canonicalFile
        root.mkdirs()
        val rootPrefix = root.path + File.separator
        val exactFiles = entry.files.toSet() + setOf("LICENSE", "README.md")
        val directoryPrefixes = entry.requiredDirectories.map { it.trimEnd('/') + "/" }

        fun shouldInstall(path: String): Boolean =
            path in exactFiles ||
                directoryPrefixes.any { prefix -> path == prefix.dropLast(1) || path.startsWith(prefix) }

        BZip2CompressorInputStream(BufferedInputStream(archive.inputStream())).use { bz ->
            TarArchiveInputStream(bz).use { tar ->
                var e = tar.nextEntry
                while (e != null) {
                    // Drop the first path segment (the wrapper dir), keep the rest.
                    val rel = e.name.substringAfter('/', e.name).trimStart('/')
                    val out = File(root, rel).canonicalFile
                    if (rel.isNotBlank() && out.path.startsWith(rootPrefix)) {
                        when {
                            e.isDirectory && shouldInstall(rel) -> out.mkdirs()
                            e.isFile && shouldInstall(rel) -> {
                                out.parentFile?.mkdirs()
                                out.outputStream().use { tar.copyTo(it) }
                            }
                            !e.isDirectory && !e.isFile -> {
                                throw IOException(
                                    ctx?.getString(R.string.voice_download_unsupported_entry) ?: "语音模型包含不支持的归档条目",
                                )
                            }
                        }
                    } else if (rel.isNotBlank()) {
                        throw IOException(ctx?.getString(R.string.voice_download_path_traversal) ?: "语音模型归档路径越界")
                    }
                    e = tar.nextEntry
                }
            }
        }
    }
}
