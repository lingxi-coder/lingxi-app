package com.lingxi.code.friends

import android.content.Intent
import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import com.lingxi.code.R
import com.lingxi.code.teams.rememberCopyToClipboard
import com.lingxi.code.theme.ConfirmDialog
import com.lingxi.code.theme.PersonAvatar
import kotlinx.coroutines.delay

@Composable
internal fun MyFriendCardScreen(
    state: FriendsState,
    showBack: Boolean,
    onClose: () -> Unit,
    onLoadProfile: () -> Unit,
    onRotateCode: () -> Unit,
    onRevokeCode: () -> Unit,
    onProfile: () -> Unit,
    onBlocked: () -> Unit,
    modifier: Modifier = Modifier,
) {
    LaunchedEffect(state.uid) { onLoadProfile() }
    Column(modifier.fillMaxSize().testTag(FriendsTags.ME_CARD)) {
        FriendsPageHeader(stringResource(R.string.friends_my_card), showBack, onClose)
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            state.profile?.let { profile ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    PersonAvatar(profile.uid, profile.displayName, 56.dp)
                    Column {
                        Text(profile.displayName, style = MaterialTheme.typography.titleLarge)
                        profile.handle?.let { Text("@$it", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                }
            }
            MyFriendLink(state, onRetry = onLoadProfile, onRotate = onRotateCode, onRevoke = onRevokeCode)
            Column {
                TextButton(onClick = onProfile, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.friends_profile_title), Modifier.weight(1f))
                    Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, contentDescription = null)
                }
                TextButton(onClick = onBlocked, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.friends_blocked_title), Modifier.weight(1f))
                    Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, contentDescription = null)
                }
            }
        }
    }
}

@Composable
private fun FriendLinkQr(link: String, modifier: Modifier = Modifier) {
    val image = remember(link) {
        val matrix = QRCodeWriter().encode(link, BarcodeFormat.QR_CODE, 288, 288)
        val pixels = IntArray(matrix.width * matrix.height) { index ->
            if (matrix[index % matrix.width, index / matrix.width]) android.graphics.Color.BLACK else android.graphics.Color.WHITE
        }
        Bitmap.createBitmap(pixels, matrix.width, matrix.height, Bitmap.Config.ARGB_8888).asImageBitmap()
    }
    Image(image, stringResource(R.string.friends_link_qr), modifier.size(204.dp).testTag(FriendsTags.LINK_QR))
}

/** *My friend link* (D6): copy, share, reset and turn off; with no link, create one. The disclosure always sits under the card. */
@Composable
private fun MyFriendLink(state: FriendsState, onRetry: () -> Unit, onRotate: () -> Unit, onRevoke: () -> Unit) {
    val profile = state.profile
    val copy = rememberCopyToClipboard(stringResource(R.string.friends_clip_link))
    val context = LocalContext.current
    val shareMessage = stringResource(R.string.friends_link_share_message)
    val shareTitle = stringResource(R.string.friends_link_share)
    var copied by remember(profile?.friendCode) { mutableStateOf(false) }
    var confirm by remember { mutableStateOf("") }
    LaunchedEffect(copied) { if (copied) { delay(2_000); copied = false } }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Surface(Modifier.fillMaxWidth().testTag(FriendsTags.LINK_CARD), shape = MaterialTheme.shapes.medium, tonalElevation = 1.dp) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(stringResource(R.string.friends_link_title), style = MaterialTheme.typography.titleSmall)
                val code = profile?.friendCode
                when {
                    profile == null && state.profileOp.error == null -> CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                    profile == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                        FriendsErrorText(state.profileOp.error!!, Modifier.weight(1f).testTag(FriendsTags.PROFILE_ERROR))
                        TextButton(onClick = onRetry) { Text(stringResource(R.string.common_retry)) }
                    }
                    code == null -> {
                        Text(stringResource(R.string.friends_link_is_off), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Button(onClick = onRotate, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_CREATE)) { Text(stringResource(R.string.friends_link_create)) }
                    }
                    else -> {
                        FriendLinkQr(code.link, Modifier.align(Alignment.CenterHorizontally))
                        SelectionContainer { Text(code.link, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.MiddleEllipsis) }
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedButton(onClick = { copy(code.link); copied = true }, modifier = Modifier.testTag(FriendsTags.LINK_COPY)) { Text(stringResource(R.string.friends_link_copy)) }
                            OutlinedButton(
                                onClick = {
                                    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "$shareMessage ${code.link}")
                                    runCatching { context.startActivity(Intent.createChooser(send, shareTitle)) }
                                },
                                modifier = Modifier.testTag(FriendsTags.LINK_SHARE),
                            ) { Text(shareTitle) }
                        }
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(onClick = { confirm = "reset" }, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_RESET)) { Text(stringResource(R.string.friends_link_reset)) }
                            TextButton(onClick = { confirm = "off" }, enabled = !state.linkOp.busy, modifier = Modifier.testTag(FriendsTags.LINK_OFF)) { Text(stringResource(R.string.friends_link_off), color = MaterialTheme.colorScheme.error) }
                        }
                        if (copied) Text(stringResource(R.string.friends_link_copied), Modifier.semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
                    }
                }
                state.linkOp.error?.let { FriendsErrorText(it) }
            }
        }
        Text(stringResource(R.string.friends_link_disclosure), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }

    if (confirm.isNotEmpty()) {
        val resetting = confirm == "reset"
        // U8: a title that names the act, what it does, a verb for the button in the error colour, and Cancel.
        ConfirmDialog(
            title = stringResource(if (resetting) R.string.friends_link_reset_title else R.string.friends_link_off_title),
            body = stringResource(if (resetting) R.string.friends_link_reset_body else R.string.friends_link_off_body),
            confirm = stringResource(if (resetting) R.string.friends_link_reset else R.string.friends_link_off), destructive = true,
            onConfirm = { if (resetting) onRotate() else onRevoke(); confirm = "" },
            onDismiss = { confirm = "" },
        )
    }
}
