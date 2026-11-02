package com.lingxi.code.conversation

import android.graphics.ImageDecoder
import android.net.Uri
import android.util.Base64
import android.webkit.MimeTypeMap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material.icons.outlined.Remove
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.nio.ByteBuffer
import kotlin.math.abs
import kotlin.math.roundToInt

internal sealed interface ImagePreviewSource {
    val mediaType: String
    data class Encoded(val base64: String, override val mediaType: String) : ImagePreviewSource
    data class LocalFile(val file: File, override val mediaType: String) : ImagePreviewSource
}

private data class PreviewAsset(val bitmap: ImageBitmap, val bytes: ByteArray, val width: Int, val height: Int)
private data class PreviewLoad(val asset: PreviewAsset? = null, val failed: Boolean = false)

@Composable
internal fun ImagePreviewDialog(source: ImagePreviewSource, onDismiss: () -> Unit, closeTag: String = "image-preview.close", previewTag: String = "image-preview") {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val load by produceState(PreviewLoad(), source) {
        value = PreviewLoad()
        value = try {
            withContext(Dispatchers.IO) {
                val bytes = when (source) {
                    is ImagePreviewSource.Encoded -> Base64.decode(source.base64, Base64.DEFAULT)
                    is ImagePreviewSource.LocalFile -> source.file.readBytes()
                }
                var width = 0
                var height = 0
                val bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
                    width = info.size.width; height = info.size.height
                    val target = imagePreviewDecodeSize(width, height)
                    decoder.setTargetSize(target.first, target.second)
                    decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                }
                PreviewLoad(PreviewAsset(bitmap.asImageBitmap(), bytes, width, height))
            }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { PreviewLoad(failed = true) }
    }
    val asset = load.asset
    var bounds by remember { mutableStateOf(IntSize.Zero) }
    var transform by remember(source) { mutableStateOf(ImagePreviewTransform()) }
    var saving by remember { mutableStateOf(false) }
    var status by remember(source) { mutableStateOf<String?>(null) }
    val geometry = ImagePreviewGeometry(asset?.width?.toFloat() ?: 0f, asset?.height?.toFloat() ?: 0f, bounds.width.toFloat(), bounds.height.toFloat())
    val fitLabel = stringResource(R.string.image_preview_fit)
    val scale = transform.scale ?: geometry.fit
    LaunchedEffect(bounds, source) { transform = ImagePreviewTransform() }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(source.mediaType)) { uri: Uri? ->
        if (uri != null && asset != null) scope.launch {
            saving = true
            status = null
            try {
                withContext(Dispatchers.IO) {
                    checkNotNull(context.contentResolver.openOutputStream(uri, "wt")).use { it.write(asset.bytes) }
                }
                status = context.getString(R.string.image_preview_saved)
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { status = context.getString(R.string.image_preview_save_failed) }
            finally { saving = false }
        }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Box(Modifier.fillMaxSize().background(Color(0xF2171719)).safeDrawingPadding().testTag(previewTag)) {
            Box(Modifier.fillMaxSize().padding(start = 12.dp, end = 12.dp, top = 76.dp, bottom = 88.dp)
                .clipToBounds().onSizeChanged { bounds = it }
                .pointerInput(geometry) {
                    detectTransformGestures { centroid, pan, zoom, _ ->
                        transform = geometry.zoom(transform, (transform.scale ?: geometry.fit) * zoom,
                            centroid.x - size.width / 2f, centroid.y - size.height / 2f, pan.x, pan.y)
                    }
                }
                .pointerInput(geometry) {
                    detectTapGestures(onDoubleTap = { point ->
                        val current = transform.scale ?: geometry.fit
                        transform = if (current > geometry.fit + .001f) ImagePreviewTransform()
                        else geometry.zoom(transform, if (geometry.fit < 1f) 1f else 2f, point.x - size.width / 2f, point.y - size.height / 2f)
                    }, onTap = { point ->
                        val current = transform.scale ?: geometry.fit
                        if (geometry.ready && (abs(point.x - size.width / 2f - transform.x) > geometry.imageWidth * current / 2 ||
                            abs(point.y - size.height / 2f - transform.y) > geometry.imageHeight * current / 2)) onDismiss()
                    })
                }, contentAlignment = Alignment.Center) {
                if (asset != null) Image(asset.bitmap, stringResource(R.string.chat_attached_image), contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxSize().graphicsLayer {
                        val base = minOf(bounds.width.toFloat() / asset.width, bounds.height.toFloat() / asset.height)
                        scaleX = scale / base; scaleY = scale / base
                        translationX = transform.x; translationY = transform.y
                    }.testTag("image-preview.image"))
                else if (load.failed) Text(stringResource(R.string.image_preview_failed), color = Color.White, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                else CircularProgressIndicator(color = Color.White)
            }
            Row(Modifier.align(Alignment.TopEnd).padding(16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                PreviewControl("image-preview.save", stringResource(R.string.image_preview_save), asset != null && !saving, {
                    val extension = MimeTypeMap.getSingleton().getExtensionFromMimeType(source.mediaType) ?: "img"
                    save.launch((source as? ImagePreviewSource.LocalFile)?.file?.name ?: "image.$extension")
                }) { Icon(Icons.Outlined.Download, null) }
                PreviewControl(closeTag, stringResource(R.string.common_close), onClick = onDismiss) { Icon(Icons.Outlined.Close, null) }
            }
            Column(Modifier.align(Alignment.BottomCenter).padding(bottom = 16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                status?.let { Text(it, color = Color.White, style = MaterialTheme.typography.labelMedium, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
                Row(Modifier.clip(CircleShape).background(Color(0xFFFAFAFA)).padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
                    PreviewControl("image-preview.zoom-out", stringResource(R.string.image_preview_zoom_out), geometry.ready && scale > geometry.minimum + .001f, { transform = geometry.step(transform, false) }) { Icon(Icons.Outlined.Remove, null) }
                    TextButton(onClick = { transform = ImagePreviewTransform() }, enabled = geometry.ready,
                        modifier = Modifier.widthIn(min = 72.dp).heightIn(min = 48.dp).testTag("image-preview.fit").semantics { contentDescription = fitLabel; stateDescription = "${(scale * 100).roundToInt()}%" }) {
                        Text(if (geometry.ready) "${(scale * 100).roundToInt()}%" else "—", color = Color(0xFF1D1D1F), modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    }
                    PreviewControl("image-preview.zoom-in", stringResource(R.string.image_preview_zoom_in), geometry.ready && scale < 8f, { transform = geometry.step(transform, true) }) { Icon(Icons.Outlined.Add, null) }
                }
            }
        }
    }
}

@Composable
private fun PreviewControl(tag: String, label: String, enabled: Boolean = true, onClick: () -> Unit, content: @Composable () -> Unit) {
    IconButton(onClick, enabled = enabled, modifier = Modifier.size(48.dp).clip(CircleShape).background(Color(0xFFFAFAFA)).testTag(tag)
        .semantics { contentDescription = label }, colors = IconButtonDefaults.iconButtonColors(contentColor = Color(0xFF1D1D1F)), content = content)
}
