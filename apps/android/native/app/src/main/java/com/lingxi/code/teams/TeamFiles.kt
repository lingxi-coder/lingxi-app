package com.lingxi.code.teams

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.InsertDriveFile
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.ByteBuffer
import java.util.UUID

// Files in a Team message (M3-4): choosing them, the unsent files above the message box, and the files of a message in the stream.
// The rules are in `TeamAttachments`; the uploads belong to `TeamsWorkspace`.

/** Reads chosen files into memory, off the main thread. Nothing is written to disk. */
internal object TeamFileReader {
    /** A picture that has to be made into a JPEG is read up to this size; a larger one is refused as too large. */
    private const val PICTURE_BYTES = 64 * 1024 * 1024

    suspend fun read(context: Context, uris: List<Uri>): List<TeamChosenFile> = withContext(Dispatchers.IO) { uris.map { read(context.contentResolver, it) } }

    private fun read(resolver: ContentResolver, uri: Uri): TeamChosenFile {
        var name = uri.lastPathSegment ?: "file"; var size = -1L
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { row ->
                if (row.moveToFirst()) { row.getString(0)?.let { name = it }; if (!row.isNull(1)) size = row.getLong(1) }
            }
        }
        // A provider that does not know a file's kind says "octet-stream": the name decides then.
        val type = resolver.getType(uri).orEmpty().let { if (it == "application/octet-stream") "" else it }
        val picture = type.startsWith("image/")
        fun refused(code: String) = TeamChosenFile(name, type, ByteArray(0), code)
        return try {
            // A file over the limit is refused before it is read.
            if (!picture && size > TeamAttachments.FILE_BYTES) return refused("ATTACHMENT_TOO_LARGE")
            val limit = if (picture) PICTURE_BYTES else TeamAttachments.FILE_BYTES
            val bytes = resolver.openInputStream(uri)?.use { stream ->
                val out = ByteArrayOutputStream(); val buffer = ByteArray(64 * 1024)
                while (true) { val count = stream.read(buffer); if (count < 0) break; out.write(buffer, 0, count); if (out.size() > limit) return refused("ATTACHMENT_TOO_LARGE") }
                out.toByteArray()
            } ?: return refused("ATTACHMENT_UNREADABLE")
            if (!picture) return TeamChosenFile(name, type, bytes)
            // A PNG or JPEG within the limit is sent as it is; any other picture (HEIC, WebP, a very large photo) is made into a JPEG.
            val kind = TeamAttachments.pictureType(bytes)
            if (kind != null && bytes.size <= TeamAttachments.FILE_BYTES) return TeamChosenFile(name, kind, bytes)
            val jpeg = TeamPictures.jpeg(bytes, 4096) ?: return refused("ATTACHMENT_TYPE")
            TeamChosenFile((if ('.' in name) name.substringBeforeLast('.') else name) + ".jpg", "image/jpeg", jpeg)
        } catch (_: Exception) { refused("ATTACHMENT_UNREADABLE") }
    }
}

/** Picture work with `ImageDecoder`, which applies the photo's orientation and never holds a whole large photo decoded. */
internal object TeamPictures {
    /** The picture no larger than [side] pixels a side, upright; null when the bytes are not a picture this device reads. */
    fun small(bytes: ByteArray, side: Int): Bitmap? = try {
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            val largest = maxOf(info.size.width, info.size.height)
            if (largest > side) { val scale = side.toDouble() / largest; decoder.setTargetSize(maxOf(1, (info.size.width * scale).toInt()), maxOf(1, (info.size.height * scale).toInt())) }
        }
    } catch (_: Exception) { null }

    fun jpeg(bytes: ByteArray, side: Int): ByteArray? {
        val bitmap = small(bytes, side) ?: return null
        return try { ByteArrayOutputStream().also { if (!bitmap.compress(Bitmap.CompressFormat.JPEG, 85, it)) return null }.toByteArray() } finally { bitmap.recycle() }
    }
}

private fun fileIcon(mime: String): ImageVector = when (mime) {
    "image/png", "image/jpeg" -> Icons.Outlined.Image
    "application/pdf" -> Icons.Outlined.PictureAsPdf
    else -> Icons.AutoMirrored.Outlined.InsertDriveFile
}

// --- the unsent files above the message box ---------------------------------------------------------

/** The "add files" button of the message box: the system's file chooser, for pictures, PDFs and text files. */
@Composable
internal fun TeamAttachButton(enabled: Boolean, onChosen: (List<TeamChosenFile>) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var reading by remember { mutableStateOf(false) }
    val chooser = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        reading = true
        scope.launch { try { onChosen(TeamFileReader.read(context.applicationContext, uris)) } finally { reading = false } }
    }
    val label = stringResource(R.string.teams_file_add)
    IconButton(enabled = enabled && !reading, onClick = { chooser.launch(TeamAttachments.CHOOSER_TYPES) }, modifier = Modifier.testTag(TeamsTags.ATTACH)) {
        if (reading) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp) else Icon(Icons.Outlined.AttachFile, label)
    }
}

/**
 * The files of the unsent message, each with where its upload stands and a button to take it out, and under them the chosen files
 * that were not taken, each with the reason.
 */
@Composable
internal fun TeamDraftFiles(files: List<TeamDraftFile>, refusals: List<TeamFileRefusal>, onRemove: (String) -> Unit, onDismissRefusals: () -> Unit) {
    if (files.isEmpty() && refusals.isEmpty()) return
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).testTag(TeamsTags.DRAFT_FILES), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        files.forEach { file ->
            val remove = stringResource(R.string.teams_file_remove_fmt, file.name)
            Row(
                Modifier.fillMaxWidth().clip(MaterialTheme.shapes.small).background(MaterialTheme.colorScheme.surfaceVariant).padding(start = 10.dp).testTag("teams.composer.file.${file.uploadId}"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(fileIcon(file.mime), null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                Column(Modifier.weight(1f).padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(file.name, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                    when (file.state) {
                        TeamDraftFile.State.Uploading -> {
                            val uploading = stringResource(R.string.teams_file_uploading)
                            LinearProgressIndicator(progress = { file.sent.toFloat() / maxOf(1, file.byteLength) }, modifier = Modifier.fillMaxWidth().semantics { contentDescription = uploading })
                        }
                        TeamDraftFile.State.Ready -> Text(TeamAttachments.size(file.byteLength), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        TeamDraftFile.State.Failed -> Text(stringResource(teamsErrorRes(file.error ?: "UPLOAD_FAILED")), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.error)
                    }
                }
                IconButton(onClick = { onRemove(file.uploadId) }) { Icon(Icons.Outlined.Close, remove, Modifier.size(18.dp)) }
            }
        }
        if (refusals.isNotEmpty()) Row(Modifier.fillMaxWidth().testTag(TeamsTags.REFUSED_FILES), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f).padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                refusals.forEach { refusal -> Text("${refusal.name}: ${stringResource(teamsErrorRes(refusal.code))}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error) }
            }
            IconButton(onClick = onDismissRefusals) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close), Modifier.size(18.dp)) }
        }
    }
}

// --- the files of a message ---------------------------------------------------------------------------

/**
 * The files a message carries. A picture is shown in the stream; any file opens in another app, from where it can be saved. The
 * bytes are asked of the service each time a file is opened, so someone who can no longer read the conversation cannot open them
 * here. The sender can take a file back; everyone then sees that it was taken back.
 *
 * [pictures] holds the small copies of the pictures shown, in memory, for as long as the conversation is open.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun TeamMessageFiles(message: TeamMessage, own: Boolean, workspace: TeamsWorkspace, pictures: SnapshotStateMap<String, ImageBitmap>) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var opening by remember(message.id) { mutableStateOf<String?>(null) }
    var failures by remember(message.id) { mutableStateOf<Map<String, String>>(emptyMap()) }
    var takingBack by remember(message.id) { mutableStateOf<TeamMessageAttachment?>(null) }
    val model = workspace.messagingModel

    fun open(file: TeamMessageAttachment) {
        if (opening != null) return
        opening = file.attachmentId; failures = failures - file.attachmentId
        scope.launch {
            try {
                val bytes = model.download(file)
                val uri = withContext(Dispatchers.IO) {
                    // One file is out at a time: whatever was opened before is deleted first.
                    workspace.openedFiles.deleteRecursively()
                    val target = File(File(workspace.openedFiles, UUID.randomUUID().toString()).apply { mkdirs() }, TeamAttachments.name(file.name))
                    target.writeBytes(bytes)
                    FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", target)
                }
                context.startActivity(Intent.createChooser(Intent(Intent.ACTION_VIEW).setDataAndType(uri, file.mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), file.name))
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { failures = failures + (file.attachmentId to ((error as? TeamMessageFailure)?.code ?: "ATTACHMENT_UNREADABLE")) }
            finally { opening = null }
        }
    }

    Column(Modifier.padding(top = 2.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        message.files.forEach { file ->
            if (file.revoked) {
                Row(Modifier.padding(horizontal = 10.dp, vertical = 6.dp).testTag("teams.message.file.${file.attachmentId}.revoked"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Icon(Icons.AutoMirrored.Outlined.InsertDriveFile, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.outline)
                    Column {
                        Text(file.name, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                        Text(stringResource(R.string.teams_file_taken_back), style = MaterialTheme.typography.labelSmall, fontStyle = FontStyle.Italic, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                return@forEach
            }
            var menu by remember(file.attachmentId) { mutableStateOf(false) }
            val openLabel = stringResource(R.string.teams_file_open_fmt, file.name)
            val takeBack = stringResource(R.string.teams_file_take_back)
            Box {
                Column(
                    Modifier
                        .combinedClickable(enabled = opening == null, onClickLabel = openLabel, onClick = { open(file) }, onLongClick = if (own) ({ menu = true }) else null)
                        .semantics { contentDescription = openLabel; if (own) customActions = listOf(CustomAccessibilityAction(takeBack) { takingBack = file; true }) }
                        .testTag("teams.message.file.${file.attachmentId}"),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (file.isImage) InlinePicture(file, model, pictures)
                    Row(
                        Modifier.clip(MaterialTheme.shapes.small).background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 10.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        if (opening == file.attachmentId) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        else Icon(fileIcon(file.mime), null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Column {
                            Text(file.name, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                            Text(TeamAttachments.size(file.byteLength), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.common_open)) }, onClick = { menu = false; open(file) })
                    DropdownMenuItem(text = { Text(takeBack, color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; takingBack = file })
                }
            }
            failures[file.attachmentId]?.let { code -> Text(stringResource(teamsErrorRes(code)), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.error) }
        }
    }
    takingBack?.let { file ->
        AlertDialog(
            onDismissRequest = { takingBack = null },
            title = { Text(stringResource(R.string.teams_file_take_back_confirm_fmt, file.name)) },
            text = { Text(stringResource(R.string.teams_file_take_back_detail)) },
            confirmButton = {
                TextButton(onClick = {
                    takingBack = null
                    scope.launch {
                        try { model.revokeAttachment(file.attachmentId) }
                        catch (error: CancellationException) { throw error }
                        catch (error: Exception) { failures = failures + (file.attachmentId to ((error as? TeamMessageFailure)?.code ?: "SERVICE_UNAVAILABLE")) }
                    }
                }) { Text(stringResource(R.string.teams_file_take_back), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { takingBack = null }) { Text(stringResource(R.string.teams_cancel)) } },
        )
    }
}

/** A picture of a message in the stream, small. It is read through the service when the message comes into view. */
@Composable
private fun InlinePicture(file: TeamMessageAttachment, model: TeamMessagingModel, pictures: SnapshotStateMap<String, ImageBitmap>) {
    val known = pictures[file.attachmentId]
    var failed by remember(file.attachmentId) { mutableStateOf(false) }
    LaunchedEffect(file.attachmentId) {
        if (pictures.containsKey(file.attachmentId)) return@LaunchedEffect
        try {
            val bytes = model.download(file)
            val small = withContext(Dispatchers.Default) { TeamPictures.small(bytes, 720) }
            if (small == null) failed = true
            else {
                // Forty pictures at most: the oldest goes first.
                while (pictures.size >= 40) pictures.remove(pictures.keys.first())
                pictures[file.attachmentId] = small.asImageBitmap()
            }
        } catch (error: CancellationException) { throw error }
        catch (_: Exception) { failed = true }
    }
    if (known != null) Image(known, null, Modifier.widthIn(max = 240.dp).heightIn(max = 240.dp).clip(MaterialTheme.shapes.medium))
    else Box(Modifier.size(160.dp, 110.dp).clip(MaterialTheme.shapes.medium).background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) {
        if (failed) Icon(Icons.Outlined.Image, null, tint = MaterialTheme.colorScheme.outline) else CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
    }
}
