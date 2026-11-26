package com.lingxi.code.conversation

import android.content.Context
import android.util.Base64
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import coil3.ImageLoader
import coil3.SingletonImageLoader
import coil3.decode.DataSource
import coil3.decode.ImageSource
import coil3.fetch.Fetcher
import coil3.fetch.SourceFetchResult
import coil3.intercept.Interceptor
import coil3.request.CachePolicy
import coil3.request.ErrorResult
import coil3.request.ImageRequest
import coil3.request.ImageResult
import coil3.request.Options
import coil3.request.SuccessResult
import coil3.size.Scale
import com.lingxi.code.model.Message
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import okio.Buffer
import okio.FileSystem

internal val chatThumbnailSize = 116.dp

/** A prefetched thumbnail and its visible row subscribe to the same Coil decode. */
internal class ChatThumbnailInterceptor : Interceptor {
    private val inFlight = ThumbnailInFlight<ImageResult>(CoroutineScope(SupervisorJob() + Dispatchers.Default))

    override suspend fun intercept(chain: Interceptor.Chain): ImageResult {
        val request = chain.request
        val key = request.memoryCacheKey
        if (request.data !is ChatImageThumbnail || key == null) return chain.proceed()

        val result = inFlight.await(key) { chain.proceed() }
        return when (result) {
            is SuccessResult -> result.copy(request = request)
            is ErrorResult -> result.copy(request = request)
        }
    }
}

internal class ThumbnailInFlight<T>(private val scope: CoroutineScope) {
    private class Work<T>(val result: Deferred<T>, var subscribers: Int = 0)

    private val active = mutableMapOf<String, Work<T>>()

    suspend fun await(key: String, load: suspend () -> T): T {
        val work = synchronized(active) {
            active.getOrPut(key) { Work(scope.async(start = CoroutineStart.LAZY) { load() }) }
                .also { it.subscribers++ }
        }
        try {
            return work.result.await()
        } finally {
            val last = synchronized(active) {
                work.subscribers--
                (work.subscribers == 0).also { if (it) active.remove(key) }
            }
            if (last) work.result.cancel()
        }
    }
}

internal class ChatImageThumbnail(
    val key: String,
    private val mediaType: String,
    private val read: suspend () -> ByteArray,
    val preview: ImagePreviewSource,
) {
    fun request(context: Context, sizePx: Int): ImageRequest = ImageRequest.Builder(context)
        .data(this)
        .fetcherFactory(ThumbnailFetcherFactory, ChatImageThumbnail::class)
        .memoryCacheKey("$key:$sizePx")
        .size(sizePx)
        .scale(Scale.FILL)
        .diskCachePolicy(CachePolicy.DISABLED)
        .build()

    private object ThumbnailFetcherFactory : Fetcher.Factory<ChatImageThumbnail> {
        override fun create(data: ChatImageThumbnail, options: Options, imageLoader: ImageLoader): Fetcher =
            object : Fetcher {
                override suspend fun fetch() = SourceFetchResult(
                    ImageSource(Buffer().write(data.read()), FileSystem.SYSTEM),
                    data.mediaType,
                    DataSource.MEMORY,
                )
            }
    }
}

internal fun chatImageThumbnails(
    message: Message,
    remoteCacheKey: String,
    loadRemoteImage: (suspend (String, Boolean) -> ByteArray)?,
): Sequence<ChatImageThumbnail> = sequence {
    for ((index, image) in message.images.withIndex()) {
        yield(ChatImageThumbnail(
            "inline:${message.transcriptUuid ?: message.id}:$index",
            image.mediaType,
            { Base64.decode(image.base64, Base64.DEFAULT) },
            ImagePreviewSource.Encoded(image.base64, image.mediaType),
        ))
    }
    if (loadRemoteImage != null) {
        for (image in message.remoteImages) {
            yield(ChatImageThumbnail(
                "remote:$remoteCacheKey:${image.url}",
                image.mediaType,
                { loadRemoteImage(image.url, true) },
                ImagePreviewSource.Remote(image.url, image.mediaType) { loadRemoteImage(image.url, false) },
            ))
        }
    }
}

@Composable
internal fun PrefetchChatThumbnails(
    items: List<ChatRenderItem>,
    listState: LazyListState,
    remoteCacheKey: String,
    loadRemoteImage: (suspend (String, Boolean) -> ByteArray)?,
) {
    val context = LocalContext.current
    val sizePx = with(LocalDensity.current) { chatThumbnailSize.roundToPx() }
    val latestItems by rememberUpdatedState(items)
    val latestLoader by rememberUpdatedState(loadRemoteImage)
    LaunchedEffect(context, listState, remoteCacheKey, sizePx, loadRemoteImage != null) {
        val loader = SingletonImageLoader.get(context)
        snapshotFlow {
            val visible = listState.layoutInfo.visibleItemsInfo
            val start = visible.firstOrNull()?.index ?: return@snapshotFlow null
            start..visible.last().index
        }.collectLatest { visible ->
            if (visible == null) return@collectLatest
            delay(64)
            val rows = latestItems
            val read = latestLoader
            val nearby = (1..4).asSequence().flatMap { distance ->
                sequenceOf(visible.first - distance, visible.last + distance)
            }
            val thumbnails = nearby.mapNotNull { index -> (rows.getOrNull(index) as? ChatRenderItem.Message)?.message }
                .flatMap { chatImageThumbnails(it, remoteCacheKey, read) }
                .take(2)
            // Keep a single speculative decode in flight. The next viewport cancels this pass.
            for (thumbnail in thumbnails) loader.execute(thumbnail.request(context, sizePx))
        }
    }
}
