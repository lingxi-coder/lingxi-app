package com.lingxi.code.conversation

import android.content.ClipData
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import androidx.compose.foundation.Image
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.ui.geometry.Offset
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.FileProvider
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import coil3.compose.AsyncImage
import com.lingxi.code.R
import com.lingxi.code.project.*
import kotlinx.coroutines.*
import java.io.File
import java.util.UUID

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun WorkspaceFilePreviewDialog(snapshot: FileSnapshot, entry: WorkspaceFileEntry, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var sharing by remember { mutableStateOf(false) }
    LaunchedEffect(snapshot) {
        try { if (entry.kind == "text") text = withContext(Dispatchers.IO) { snapshot.textPreview() } }
        catch (cause: WorkspaceDirectoryException) { error = context.getString(cause.messageResource) }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (cause: Exception) { error = cause.message }
        finally { loading = false }
    }
    fun openExternal(share: Boolean) {
        if (sharing) return
        sharing = true
        scope.launch {
            val directory = File(context.cacheDir, "workspace-file-share/${UUID.randomUUID()}")
            var handedToSystem = false
            try {
                val file = withContext(Dispatchers.IO) {
                    check(directory.mkdirs())
                    val copy = File(directory, snapshot.file.name)
                    try { copyWorkspaceTree(snapshot.file, copy); copy } catch (e: Exception) { directory.deleteRecursively(); throw e }
                }
                val uri = FileProvider.getUriForFile(context, context.packageName + ".fileprovider", file)
                val intent = if (share) Intent(Intent.ACTION_SEND).setType(fileMime(file.name)).putExtra(Intent.EXTRA_STREAM, uri)
                    else Intent(Intent.ACTION_VIEW).setDataAndType(uri, fileMime(file.name))
                intent.clipData = ClipData.newRawUri(file.name, uri)
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                context.startActivity(Intent.createChooser(intent, context.getString(if (share) R.string.files_share else R.string.files_open_with)))
                handedToSystem = true
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (cause: Exception) { error = cause.message }
            finally {
                if (!handedToSystem) withContext(NonCancellable + Dispatchers.IO) { directory.deleteRecursively() }
                sharing = false
            }
        }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            Scaffold(topBar = { TopAppBar(title = { Text(entry.name, maxLines = 1) }, navigationIcon = {
                IconButton(onClick = onDismiss, modifier = Modifier.testTag("files.preview.close")) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.files_close)) }
            }, actions = { IconButton(onClick = { openExternal(true) }, enabled = !sharing) { Icon(Icons.Outlined.Share, stringResource(R.string.files_share)) } }) }) { padding ->
                Column(Modifier.padding(padding).fillMaxSize().testTag("files.preview")) {
                    if (loading) LinearProgressIndicator(Modifier.fillMaxWidth())
                    if (error != null) Text(error!!, Modifier.padding(16.dp))
                    else when (entry.kind) {
                        "text" -> if (text != null) {
                            val lines = remember(text) { text!!.splitToSequence('\n').flatMap { line -> if (line.isEmpty()) sequenceOf("") else line.chunked(4096).asSequence() }.toList() }
                            LazyColumn(Modifier.weight(1f).testTag("files.text-preview"), contentPadding = PaddingValues(16.dp)) {
                                items(lines.size) { index -> SelectionContainer { Text(lines[index], fontFamily = FontFamily.Monospace, modifier = Modifier.fillMaxWidth()) } }
                            }
                        }
                        "image" -> {
                            var zoom by remember { mutableFloatStateOf(1f) }
                            var pan by remember { mutableStateOf(Offset.Zero) }
                            val transform = rememberTransformableState { scale, offset, _ -> zoom = (zoom * scale).coerceIn(1f, 5f); pan = if (zoom == 1f) Offset.Zero else pan + offset }
                            Box(Modifier.weight(1f).fillMaxWidth().transformable(transform), contentAlignment = Alignment.Center) {
                                AsyncImage(snapshot.file, entry.name, modifier = Modifier.fillMaxSize().graphicsLayer { scaleX = zoom; scaleY = zoom; translationX = pan.x; translationY = pan.y })
                            }
                        }
                        "media" -> WorkspaceMediaPreview(snapshot.file, Modifier.weight(1f))
                        "pdf" -> WorkspacePdfPreview(snapshot.file, Modifier.weight(1f)) { error = it }
                        else -> { Text(stringResource(R.string.files_unsupported), Modifier.padding(16.dp)); Text(entry.name, Modifier.padding(16.dp)) }
                    }
                    TextButton(onClick = { openExternal(false) }, enabled = !sharing) { Text(stringResource(R.string.files_open_with)) }
                }
            }
        }
    }
}
@Composable private fun WorkspaceMediaPreview(file: File, modifier: Modifier) {
    val context = LocalContext.current
    val player = remember(file) { ExoPlayer.Builder(context).build().apply { setMediaItem(MediaItem.fromUri(android.net.Uri.fromFile(file))); prepare() } }
    val owner = LocalLifecycleOwner.current
    DisposableEffect(player, owner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) player.pause() }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer); player.release() }
    }
    AndroidView(factory = { PlayerView(it).apply { this.player = player } }, modifier = modifier.fillMaxWidth())
}
@Composable private fun WorkspacePdfPreview(file: File, modifier: Modifier, onError: (String) -> Unit) {
    var count by remember(file) { mutableIntStateOf(0) }
    LaunchedEffect(file) {
        try { count = withContext(Dispatchers.IO) { ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor -> PdfRenderer(descriptor).use { it.pageCount } } } }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) { onError(error.message.orEmpty()) }
    }
    LazyColumn(modifier.fillMaxWidth()) {
        items((0 until count).toList(), key = { it }) { index ->
            var bitmap by remember(file, index) { mutableStateOf<Bitmap?>(null) }
            LaunchedEffect(file, index) {
                try {
                    bitmap = withContext(Dispatchers.IO) {
                        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor ->
                            PdfRenderer(descriptor).use { renderer -> renderer.openPage(index).use { page ->
                                val scale = minOf(1.5f, 1600f / maxOf(page.width, page.height))
                                Bitmap.createBitmap(maxOf(1, (page.width * scale).toInt()), maxOf(1, (page.height * scale).toInt()), Bitmap.Config.ARGB_8888).also {
                                    it.eraseColor(android.graphics.Color.WHITE); page.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                                }
                            } }
                        }
                    }
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (error: Exception) { onError(error.message.orEmpty()) }
            }
            Box(Modifier.fillMaxWidth().heightIn(min = 400.dp), contentAlignment = Alignment.Center) {
                bitmap?.let { Image(it.asImageBitmap(), "${index + 1}/$count", Modifier.fillMaxWidth().padding(8.dp)) } ?: CircularProgressIndicator()
            }
        }
    }
}
