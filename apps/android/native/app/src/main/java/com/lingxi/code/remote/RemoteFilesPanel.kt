package com.lingxi.code.remote

import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteFileFfi
import com.lingxi.code.bindings.android.RemoteTransportFfi
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Original-byte transfer is separate from the camera/image-compression picker. */
@Composable
internal fun RemoteFilesPanel(manager: RemoteControlManager, hostId: String, sessionId: String, enabled: Boolean) {
    val uid by manager.uid.collectAsState()
    androidx.compose.runtime.key(uid, hostId, sessionId) {
        RemoteFilesContent(manager, hostId, sessionId, enabled)
    }
}

@Composable
private fun RemoteFilesContent(manager: RemoteControlManager, hostId: String, sessionId: String, enabled: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val uid by manager.uid.collectAsState()
    val connection by manager.connection.collectAsState()
    val owner = uid
    val key = "$owner/$hostId/$sessionId"
    val available = enabled && connection?.transport == RemoteTransportFfi.WEBRTC
    var expanded by remember(key) { mutableStateOf(false) }
    var busy by remember(key) { mutableStateOf(false) }
    var files by remember(key) { mutableStateOf(emptyList<RemoteFileFfi>()) }
    var source by remember(key) { mutableStateOf<File?>(null) }
    var prepared by remember(key) { mutableStateOf<RemoteFileFfi?>(null) }
    var transfer by remember(key) { mutableStateOf<String?>(null) }
    var total by remember(key) { mutableStateOf(0UL) }
    var committed by remember(key) { mutableStateOf(0UL) }
    var problem by remember(key) { mutableStateOf<String?>(null) }
    var operation by remember(key) { mutableStateOf<Job?>(null) }
    var pendingDownload by remember(key) { mutableStateOf<RemoteFileFfi?>(null) }
    val root = remember(key) {
        val hash = MessageDigest.getInstance("SHA-256").digest(key.toByteArray()).joinToString("") { "%02x".format(it) }
        File(context.cacheDir, "remote-files/$hash")
    }
    val sessionReady = rememberUpdatedState(enabled)
    suspend fun client() = manager.filePort(owner, hostId, snapshotFlow { sessionReady.value })
    suspend fun <T> fileOperation(action: suspend (RemoteClientPort) -> T): T =
        manager.fileOperation(owner, hostId, snapshotFlow { sessionReady.value }, action)
    suspend fun upload() {
        val selected = source ?: return
        busy = true; problem = null; committed = 0UL
        try {
            transfer = selected.absolutePath
            val file = prepared ?: fileOperation { it.prepareFile(sessionId, selected.absolutePath, selected.name) }.also { prepared = it }
            transfer = file.transferId; total = file.size
            val receipt = fileOperation { it.uploadFile(file, selected.absolutePath) }
            client() // Recheck the account/host after suspension before publishing UI state.
            files = files.filterNot { it.transferId == receipt.transferId } + receipt
            source = null; prepared = null
            withContext(Dispatchers.IO) { selected.parentFile?.deleteRecursively() }
        } catch (error: CancellationException) { throw error }
        catch (error: Exception) { problem = error.message }
        finally { busy = false; transfer = null }
    }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val file = pendingDownload
        pendingDownload = null
        if (uri == null || file == null) busy = false
        else operation = scope.launch {
            problem = null; committed = 0UL; total = file.size; transfer = file.transferId
            try {
                val output = withContext(Dispatchers.IO) {
                    check(file.transferId.matches(Regex("[0-9a-f]{32}")))
                    root.mkdirs()
                    File(root, file.transferId)
                }
                fileOperation { it.downloadFile(file, output.absolutePath) }
                withContext(Dispatchers.IO) {
                    context.contentResolver.openOutputStream(uri, "wt").use { target ->
                        checkNotNull(target) { "Cannot open destination" }
                        output.inputStream().use { input ->
                            val buffer = ByteArray(64 * 1024)
                            while (true) {
                                coroutineContext.ensureActive()
                                val count = input.read(buffer)
                                if (count < 0) break
                                target.write(buffer, 0, count)
                            }
                        }
                    }
                }
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { problem = error.message }
            finally { busy = false; transfer = null }
        }
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) busy = false
        else operation = scope.launch {
            // Import/reconnect can take time. Clear the previous file's progress.
            problem = null; committed = 0UL; total = 0UL; transfer = null
            var staging: File? = null
            var retained = false
            try {
                val imported = withContext(Dispatchers.IO) {
                    val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                        if (cursor.moveToFirst()) cursor.getString(0) else null
                    } ?: "file"
                    require(name.isNotBlank() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\'))
                    val directory = File(root, UUID.randomUUID().toString()).apply { check(mkdirs()) }
                    staging = directory
                    val result = File(directory, name)
                    context.contentResolver.openInputStream(uri).use { input ->
                        checkNotNull(input) { "Cannot open selected file" }
                        result.outputStream().use { output ->
                            val buffer = ByteArray(64 * 1024)
                            while (true) {
                                coroutineContext.ensureActive()
                                val count = input.read(buffer)
                                if (count < 0) break
                                output.write(buffer, 0, count)
                            }
                        }
                    }
                    result
                }
                client()
                source?.parentFile?.deleteRecursively()
                source = imported; prepared = null; retained = true; upload()
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { problem = error.message }
            finally {
                if (!retained) staging?.deleteRecursively()
                busy = false
            }
        }
    }
    LaunchedEffect(key, available) { if (available) runCatching { client().listFiles(sessionId) }.onSuccess { files = it } }
    LaunchedEffect(key, transfer) {
        val id = transfer ?: return@LaunchedEffect
        while (busy) { committed = runCatching { client().fileProgress(id) }.getOrDefault(committed); delay(300) }
    }
    Column(Modifier.padding(horizontal = 12.dp)) {
        TextButton(onClick = { expanded = !expanded }) { Text(stringResource(R.string.remote_files)) }
        if (expanded) {
            Row {
                TextButton(enabled = available && !busy, onClick = { if (!busy) { busy = true; picker.launch(arrayOf("*/*")) } }) { Text(stringResource(R.string.remote_file_upload)) }
                if (source != null && !busy) TextButton(enabled = available, onClick = { if (!busy) { busy = true; operation = scope.launch { upload() } } }) { Text(stringResource(R.string.common_retry)) }
                if (busy) TextButton(onClick = { operation?.cancel() }) { Text(stringResource(R.string.remote_file_cancel)) }
            }
            if (busy) {
                LinearProgressIndicator(progress = { if (total > 0UL) (committed.toDouble() / total.toDouble()).toFloat() else 0f })
                Text("$committed / $total B")
            }
            problem?.let { Text(it) }
            files.forEach { file -> Row {
                Text(file.name, modifier = Modifier.weight(1f), maxLines = 1)
                TextButton(enabled = available && !busy, onClick = { if (!busy) {
                    busy = true
                    operation = scope.launch {
                        try { client().sendPromptWithFiles(sessionId, file.name, emptyList(), listOf(file.fileRef)) }
                        catch (error: CancellationException) { throw error }
                        catch (error: Exception) { problem = error.message }
                        finally { busy = false }
                    }
                } }) { Text(stringResource(R.string.remote_file_attach)) }
                TextButton(enabled = available && !busy, onClick = { if (!busy) { busy = true; pendingDownload = file; save.launch(file.name) } }) { Text(stringResource(R.string.remote_file_download)) }
            } }
            if (!available) Text(stringResource(R.string.remote_file_requires_webrtc))
        }
    }
}
