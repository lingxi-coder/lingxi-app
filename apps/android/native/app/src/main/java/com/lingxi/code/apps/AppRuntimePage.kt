package com.lingxi.code.apps

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.view.ViewGroup
import android.webkit.WebView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** Links an app may hand to the system; anything else (intent:, file:, javascript:) is dropped. */
private val ExternalSchemes = setOf("https", "http", "mailto", "tel")

/**
 * An installed app running full screen. The market's runtime owns the WebView
 * (the app's own origin and storage profile, no native bridge); this page only
 * attaches it, pauses it with the activity and closes it on the way out. The
 * only chrome is the capsule in the top-right corner, as in a mini program.
 */
@Composable
fun AppRuntimePage(
    localApps: LocalApps,
    appId: String,
    onClose: () -> Unit,
    onShowDetails: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val context = LocalContext.current
    val runtime = localApps.market.runtime
    var webView by remember(appId) { mutableStateOf<WebView?>(null) }
    var openError by remember(appId) { mutableStateOf<String?>(null) }
    var externalUrl by remember(appId) { mutableStateOf<String?>(null) }
    var menuOpen by remember { mutableStateOf(false) }

    LaunchedEffect(appId) {
        try {
            webView = runtime.open(appId) { url ->
                if (Uri.parse(url).scheme?.lowercase() in ExternalSchemes) externalUrl = url
            }
            localApps.info.markOpened(appId, System.currentTimeMillis())
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            openError = marketErrorCode(e)
        }
    }
    // The runtime keeps one WebView per app until it is closed. Closing when
    // the page goes away means the next open starts the app fresh from its
    // files, and a recreated activity never receives a view still attached to
    // the old one: the market scope runs on Main.immediate, so this close has
    // finished before anything else composes.
    DisposableEffect(appId) {
        onDispose { localApps.scope.launch { runtime.close(appId) } }
    }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner, appId) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_PAUSE -> localApps.scope.launch { runtime.suspend(appId) }
                Lifecycle.Event.ON_RESUME -> localApps.scope.launch { runtime.resume(appId) }
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    BackHandler {
        val view = webView
        if (view != null && view.canGoBack()) view.goBack() else onClose()
    }

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(t.windowBg)
            .blockTouchesBehind()
            .windowInsetsPadding(WindowInsets.systemBars)
            .testTag(UiTags.APP_RUNTIME_PAGE),
    ) {
        val view = webView
        val error = openError
        when {
            view != null -> key(view) {
                AndroidView(
                    factory = { view },
                    modifier = Modifier.fillMaxSize(),
                    onRelease = { released -> (released.parent as? ViewGroup)?.removeView(released) },
                )
            }
            error != null -> Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier
                    .align(Alignment.Center)
                    .padding(horizontal = 32.dp),
            ) {
                Text(
                    text = if (error.endsWith("_UNSUPPORTED")) {
                        stringResource(R.string.apps_webview_unsupported, error)
                    } else {
                        stringResource(R.string.apps_open_failed_fmt, error)
                    },
                    color = t.text2,
                    fontSize = 14.sp,
                )
                Text(
                    text = stringResource(R.string.common_close),
                    color = t.accent,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .padding(top = 14.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(onClick = onClose)
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                )
            }
            else -> CircularProgressIndicator(
                modifier = Modifier
                    .align(Alignment.Center)
                    .size(26.dp),
                color = t.text3,
                strokeWidth = 2.5.dp,
            )
        }

        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(top = 8.dp, end = 10.dp),
        ) {
            Capsule(onMore = { menuOpen = true }, onClose = onClose)
            DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.apps_details)) },
                    onClick = {
                        menuOpen = false
                        onShowDetails(appId)
                    },
                )
            }
        }
    }

    externalUrl?.let { url ->
        AlertDialog(
            onDismissRequest = { externalUrl = null },
            title = { Text(stringResource(R.string.apps_external_link_title)) },
            text = { Text(stringResource(R.string.apps_external_link_body_fmt, url)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        externalUrl = null
                        try {
                            context.startActivity(
                                Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE),
                            )
                        } catch (_: ActivityNotFoundException) {
                            // Nothing on the device handles this link; there is nothing to open.
                        }
                    },
                ) { Text(stringResource(R.string.common_open)) }
            },
            dismissButton = {
                TextButton(onClick = { externalUrl = null }) { Text(stringResource(R.string.common_cancel)) }
            },
        )
    }
}

/** The mini-program capsule: more (⋯) and close (✕). */
@Composable
private fun Capsule(onMore: () -> Unit, onClose: () -> Unit) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(50)
    val moreLabel = stringResource(R.string.composer_more)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .height(34.dp)
            .clip(shape)
            .background(t.surface.copy(alpha = 0.92f))
            .border(0.5.dp, t.border, shape),
    ) {
        Row(
            horizontalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .width(46.dp)
                .height(34.dp)
                .semantics { contentDescription = moreLabel }
                .testTag(UiTags.APP_RUNTIME_MORE)
                .clickable(onClick = onMore),
        ) {
            repeat(3) { Box(Modifier.size(4.dp).clip(CircleShape).background(t.text)) }
        }
        Box(Modifier.width(0.5.dp).height(16.dp).background(t.border))
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier
                .width(46.dp)
                .height(34.dp)
                .testTag(UiTags.APP_RUNTIME_CLOSE)
                .clickable(onClick = onClose),
        ) {
            LXIcon(
                name = LXIconName.X,
                size = 15.dp,
                color = t.text,
                stroke = 2f,
                contentDescription = stringResource(R.string.common_close),
            )
        }
    }
}
