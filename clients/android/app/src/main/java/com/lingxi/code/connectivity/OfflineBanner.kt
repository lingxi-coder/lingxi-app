package com.lingxi.code.connectivity

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.tint
import com.lingxi.code.theme.LingXiTheme

/**
 * A dismissible offline banner — surfaced in the chat scaffold when the device
 * has no validated network ([com.lingxi.code.connectivity.ConnectivityObserver]).
 *
 * It only INFORMS; it never hard-blocks the composer or the conversation (the
 * engine queues / errors on its own). The banner animates in/out, offers a
 * "重试" affordance (e.g. to re-kick a connection attempt), and a × to dismiss.
 * Dismissal is owned by the caller via [onDismiss] so the visibility logic
 * (offline AND not-dismissed) stays hoisted and testable; [shouldShowOfflineBanner]
 * is the pure predicate.
 *
 * @param visible whether to show the banner (caller computes offline && !dismissed).
 * @param onRetry optional retry affordance (hidden when null).
 * @param onDismiss dismiss (×) action.
 */
@Composable
fun OfflineBanner(
    visible: Boolean,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    onRetry: (() -> Unit)? = null,
) {
    val t = LingXiTheme.palette
    AnimatedVisibility(
        visible = visible,
        enter = expandVertically() + fadeIn(),
        exit = shrinkVertically() + fadeOut(),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            modifier = modifier
                .testTag(UiTags.OFFLINE_BANNER)
                .fillMaxWidth()
                .padding(horizontal = 14.dp)
                .padding(top = 6.dp, bottom = 2.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(t.statusError.tint(0.12f))
                .border(0.5.dp, t.statusError.tint(0.4f), RoundedCornerShape(12.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(18.dp)
                    .clip(CircleShape)
                    .background(t.statusError),
                contentAlignment = Alignment.Center,
            ) {
                LXIcon(name = LXIconName.X, size = 11.dp, color = Color.White, stroke = 2.4f)
            }
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = "已离线",
                    color = t.statusError,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "网络连接不可用，部分功能可能受影响。",
                    color = t.text2,
                    fontSize = 12.5f.sp,
                    lineHeight = (12.5f * 1.4f).sp,
                )
            }
            if (onRetry != null) {
                Text(
                    text = "重试",
                    color = t.accent,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(onClick = onRetry)
                        .testTag(UiTags.OFFLINE_RETRY)
                        .padding(horizontal = 10.dp, vertical = 6.dp),
                )
            }
            Box(
                modifier = Modifier
                    .size(28.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(onClick = onDismiss)
                    .testTag(UiTags.OFFLINE_DISMISS),
                contentAlignment = Alignment.Center,
            ) {
                LXIcon(name = LXIconName.X, size = 14.dp, color = t.text3, stroke = 2f, contentDescription = "关闭离线提示")
            }
        }
    }
}

/**
 * PURE visibility predicate for the offline banner: show it only when the device
 * is offline AND the user has not dismissed the current offline episode. Coming
 * back online resets dismissal (the caller drops [dismissedWhileOffline] to
 * false on the offline→online edge) so the next disconnect re-shows the banner.
 */
fun shouldShowOfflineBanner(isOnline: Boolean, dismissedWhileOffline: Boolean): Boolean =
    !isOnline && !dismissedWhileOffline
