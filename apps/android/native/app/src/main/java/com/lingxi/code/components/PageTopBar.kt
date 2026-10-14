package com.lingxi.code.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme

/** The smallest comfortable thumb target; Material's own minimum is 48dp. */
private val TouchTarget = 44.dp

/**
 * The top bar of a full-screen page pushed over the conversation (projects,
 * a project): back chevron on the left, centered title, optional trailing
 * action. Same look as the settings top bar, with thumb-sized targets at both
 * ends.
 */
@Composable
fun PageTopBar(
    title: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    trailing: (@Composable () -> Unit)? = null,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier
            .fillMaxWidth()
            .drawBehind {
                drawLine(
                    color = t.border,
                    start = Offset(0f, size.height),
                    end = Offset(size.width, size.height),
                    strokeWidth = 0.5.dp.toPx(),
                )
            }
            .padding(horizontal = 4.dp)
            .padding(top = 2.dp, bottom = 6.dp),
    ) {
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier
                .size(TouchTarget)
                .clip(RoundedCornerShape(10.dp))
                .clickable(onClick = onBack),
        ) {
            LXIcon(
                name = LXIconName.ChevronR,
                size = 17.dp,
                color = t.text2,
                stroke = 2.2f,
                modifier = Modifier.rotate(180f),
                contentDescription = stringResource(R.string.onboarding_back),
            )
        }
        Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
            Text(
                text = title,
                color = t.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
            )
        }
        // Always as wide as the back target, so the title stays centered.
        Box(Modifier.widthIn(min = TouchTarget), contentAlignment = Alignment.Center) {
            trailing?.invoke() ?: Box(Modifier.size(TouchTarget))
        }
    }
}

/**
 * A page drawn over the conversation owns every touch inside its bounds.
 * Without a pointer-input node an unhandled tap falls through to whatever is
 * underneath — the chat's title (which opens the session panel) or its composer.
 * A Material `Surface` does the same, which is why settings never needed this.
 */
fun Modifier.blockTouchesBehind(): Modifier = pointerInput(Unit) {}
