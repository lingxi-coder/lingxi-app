package com.lingxi.code.voice.offline

import android.content.Context
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream
import java.io.BufferedInputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import kotlin.coroutines.coroutineContext

/**
 * Downloads sherpa-onnx voice models on demand (the setup wizard's language-pack
 * step). Self-contained port of the relevant slice of ~/lingxi/android's
 * OfflineModelManager: stream the `.tar.bz2` with progress → verify SHA-256 →
 * extract the declared files (flattened) into `filesDir/voice_models/<id>/`.
 *
 * Process-global (attached with the application Context in MainActivity, like the
 * Camera/Share/etc. controllers) so a download survives leaving the wizard step.
 * The native sherpa runtime that USES these models lands in a later phase; this
 * just stages them on disk and exposes per-model [ModelState].
 */
object VoiceModelDownloader {
    private var appContext: Context? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val jobs = mutableMapOf<String, Job>()

    private val _states = MutableStateFlow<Map<String, ModelState>>(emptyMap())
    val states: StateFlow<Map<String, ModelState>> = _states.asStateFlow()

    fun attach(context: Context) {
        appContext = context.applicationContext
        reconcileFromDisk()
    }

    fun modelDir(id: String): File = File(appContext!!.filesDir, "voice_models/$id")

    fun isReady(entry: OfflineModelEntry): Boolean =
        appContext != null && entry.files.all { File(modelDir(entry.id), it).exists() }

    /** Mark already-on-disk models Ready so re-entering the wizard reflects reality. */
    private fun reconcileFromDisk() {
        _states.value = OfflineModelCatalog.all.associate {
            it.id to if (isReady(it)) ModelState.Ready else (_states.value[it.id] ?: ModelState.NotInstalled)
        }
    }

    /** Start (or resume) downloading every model in a language pack. */
    fun startPack(language: String) {
        OfflineModelCatalog.packFor(language).forEach { start(it) }
    }

    fun start(entry: OfflineModelEntry) {
        when (_states.value[entry.id]) {
            is ModelState.Ready, is ModelState.Downloading,
            is ModelState.Verifying, is ModelState.Extracting -> return
            else -> {}
        }
        jobs[entry.id]?.cancel()
        jobs[entry.id] = scope.launch { runDownload(entry) }
    }

    fun cancel(entry: OfflineModelEntry) {
        jobs[entry.id]?.cancel()
        set(entry.id, if (isReady(entry)) ModelState.Ready else ModelState.NotInstalled)
    }

    private fun set(id: String, state: ModelState) = _states.update { it + (id to state) }

    private suspend fun runDownload(entry: OfflineModelEntry) {
        val ctx = appContext ?: return
        val tmp = File(ctx.cacheDir, "voice_dl/${entry.id}.tar.bz2")
        try {
            tmp.parentFile?.mkdirs()
            set(entry.id, ModelState.Downloading(0, entry.approxSizeBytes))
            val digest = MessageDigest.getInstance("SHA-256")
            val conn = (URL(entry.sourceUrl).openConnection() as HttpURLConnection).apply {
                connectTimeout = 30_000; readTimeout = 60_000; instanceFollowRedirects = true
            }
            conn.connect()
            val total = if (conn.contentLengthLong > 0) conn.contentLengthLong else entry.approxSizeBytes
            conn.inputStream.use { input ->
                tmp.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    var read = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        digest.update(buf, 0, n)
                        read += n
                        set(entry.id, ModelState.Downloading(read, total))
                        coroutineContext.ensureActive()
                    }
                }
            }
            set(entry.id, ModelState.Verifying)
            val hex = digest.digest().joinToString("") { "%02x".format(it) }
            if (!hex.equals(entry.sha256, ignoreCase = true)) {
                tmp.delete(); set(entry.id, ModelState.Failed("校验失败 (sha256 不匹配)")); return
            }
            set(entry.id, ModelState.Extracting)
            extract(tmp, entry, modelDir(entry.id))
            tmp.delete()
            set(entry.id, if (isReady(entry)) ModelState.Ready else ModelState.Failed("解压后文件缺失"))
        } catch (ce: CancellationException) {
            tmp.delete(); set(entry.id, ModelState.NotInstalled); throw ce
        } catch (e: Throwable) {
            tmp.delete(); set(entry.id, ModelState.Failed(e.message ?: "下载失败"))
        }
    }

    /** Extract the entry's declared files (flattened — archives nest under a top dir). */
    private fun extract(archive: File, entry: OfflineModelEntry, destDir: File) {
        destDir.mkdirs()
        val want = entry.files.toSet()
        BZip2CompressorInputStream(BufferedInputStream(archive.inputStream())).use { bz ->
            TarArchiveInputStream(bz).use { tar ->
                var e = tar.nextEntry
                while (e != null) {
                    if (!e.isDirectory) {
                        val base = File(e.name).name
                        if (base in want) {
                            File(destDir, base).outputStream().use { tar.copyTo(it) }
                        }
                    }
                    e = tar.nextEntry
                }
            }
        }
    }
}
