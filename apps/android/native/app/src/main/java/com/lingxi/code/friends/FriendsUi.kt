package com.lingxi.code.friends

import android.icu.text.ListFormatter
import android.icu.text.RelativeDateTimeFormatter
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import kotlinx.coroutines.delay
import java.util.Locale

/** Test tags of the Friends tab. */
internal object FriendsTags {
    const val ROOT = "friends.root"
    const val BACK = "friends.back"
    const val SIGN_IN = "friends.signin"
    const val LOADING = "friends.loading"
    const val LOAD_FAILED = "friends.loadFailed"
    const val EMPTY = "friends.empty"
    const val LIST = "friends.list"
    const val ADD = "friends.add"
    const val MORE = "friends.more"
    const val SEARCH = "friends.search"
    const val FILTER_FIELD = "friends.filterField"
    const val FILTER_EMPTY = "friends.filterEmpty"
    const val NAME_BANNER = "friends.nameBanner"
    const val PROFILE_ERROR = "friends.profileError"
    const val REQUESTS = "friends.requests"
    const val NOTICE = "friends.notice"
    const val ADD_SCREEN = "friends.addScreen"
    const val LINK_CARD = "friends.linkCard"
    const val LINK_COPY = "friends.linkCopy"
    const val LINK_SHARE = "friends.linkShare"
    const val LINK_RESET = "friends.linkReset"
    const val LINK_OFF = "friends.linkOff"
    const val LINK_CREATE = "friends.linkCreate"
    const val SEARCH_INPUT = "friends.searchInput"
    const val SEARCH_ACTION = "friends.searchAction"
    const val SEARCH_ERROR = "friends.searchError"
    const val RESULT = "friends.result"
    const val RESULT_ACTION = "friends.resultAction"
    const val RESULT_NOTE = "friends.resultNote"
    const val LINK_DIALOG = "friends.linkDialog"
    const val PROFILE_SCREEN = "friends.profileScreen"
    const val PROFILE_NAME = "friends.profileName"
    const val PROFILE_HANDLE = "friends.profileHandle"
    const val PROFILE_SAVE = "friends.profileSave"
    const val BLOCKED_SCREEN = "friends.blockedScreen"
    const val BLOCKED_EMPTY = "friends.blockedEmpty"
    const val CHAT = "friends.chat"
    const val CHAT_UNAVAILABLE = "friends.chatUnavailable"
    const val CHAT_MENU = "friends.chatMenu"
    const val OFFERS = "friends.offers"
    const val BOTS = "friends.bots"
    const val BOTS_SCREEN = "friends.botsScreen"
    const val BOTS_STATUS = "friends.botsStatus"
    const val BOTS_NOTE = "friends.botsNote"
    const val BOTS_SHARE_HINT = "friends.botsShareHint"
    const val INVITE_SHEET = "friends.inviteSheet"
    const val INVITE_FILTER = "friends.inviteFilter"
    const val INVITE_EMPTY = "friends.inviteEmpty"
    const val INVITE_FILTER_EMPTY = "friends.inviteFilterEmpty"
    const val INVITE_DISCLOSURE = "friends.inviteDisclosure"
    fun row(uid: String) = "friends.row.$uid"
    fun accept(uid: String) = "friends.accept.$uid"
    fun decline(uid: String) = "friends.decline.$uid"
    fun cancel(uid: String) = "friends.cancel.$uid"
    fun unblock(uid: String) = "friends.unblock.$uid"
    fun offer(teamId: String) = "friends.offer.$teamId"
    fun offerExpiry(teamId: String) = "friends.offerExpiry.$teamId"
    fun offerJoin(teamId: String) = "friends.offerJoin.$teamId"
    fun offerDecline(teamId: String) = "friends.offerDecline.$teamId"
    fun offerDismiss(teamId: String) = "friends.offerDismiss.$teamId"
    fun bot(sharedBotId: String) = "friends.bot.$sharedBotId"
    fun botMessage(sharedBotId: String) = "friends.botMessage.$sharedBotId"
    fun botStop(sharedBotId: String) = "friends.botStop.$sharedBotId"
    fun invite(uid: String) = "friends.invite.$uid"
    fun inviteSend(uid: String) = "friends.inviteSend.$uid"
}

/** An error as the person reads it (D18). */
@StringRes
internal fun friendsErrorRes(error: FriendsError): Int = when (error) {
    FriendsError.NotFound -> R.string.friends_error_not_found
    FriendsError.RateLimited -> R.string.friends_error_rate_limited
    FriendsError.FriendLimit -> R.string.friends_error_friend_limit
    FriendsError.RequestLimit -> R.string.friends_error_request_limit
    FriendsError.HandleTaken -> R.string.friends_error_handle_taken
    FriendsError.RequestUnavailable -> R.string.friends_error_request_unavailable
    FriendsError.StaleRequest -> R.string.friends_error_stale_request
    FriendsError.Session -> R.string.friends_error_session
    FriendsError.Network -> R.string.friends_error_network
    FriendsError.InvalidInput -> R.string.friends_error_invalid_input
    FriendsError.ChatUnavailable -> R.string.friends_chat_unavailable
    FriendsError.FriendRequired -> R.string.friends_error_friend_required
    FriendsError.AlreadyMember -> R.string.friends_error_already_member
    FriendsError.OfferLimit -> R.string.friends_error_offer_limit
    FriendsError.MemberLimit -> R.string.friends_error_member_limit
    FriendsError.TeamLimit -> R.string.friends_error_team_limit
    FriendsError.OfferRole -> R.string.friends_error_offer_role
    FriendsError.OfferTeamGone -> R.string.friends_error_offer_team_gone
    FriendsError.OfferUnavailable -> R.string.friends_error_offer_unavailable
    FriendsError.Generic -> R.string.friends_error_generic
}

/** The message that belongs under the control that failed. */
@Composable
internal fun FriendsErrorText(error: FriendsError, modifier: Modifier = Modifier, tag: String? = null) {
    Text(
        stringResource(friendsErrorRes(error)), modifier.then(if (tag != null) Modifier.testTag(tag) else Modifier),
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error,
    )
}

/** The header of a pushed page (or a pane when wide): Back on phones, Close beside other panes. */
@Composable
internal fun FriendsPageHeader(title: String, showBack: Boolean, onClose: () -> Unit) {
    Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onClose, modifier = Modifier.testTag(FriendsTags.BACK)) {
            if (showBack) Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.remote_back)) else Icon(Icons.Outlined.Close, stringResource(R.string.common_close))
        }
        Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
    }
}

/** A line the Friends tab shows for a few seconds (D4, D7, D16, D27); a tap dismisses it. */
@Composable
internal fun FriendsNoticeBanner(notice: FriendsNotice, onDismiss: () -> Unit, modifier: Modifier = Modifier) {
    val text = when (notice) {
        is FriendsNotice.NowFriends -> stringResource(R.string.friends_now_friends_fmt, notice.name)
        FriendsNotice.ChatUnavailable -> stringResource(R.string.friends_chat_unavailable)
        is FriendsNotice.OfferJoined -> stringResource(R.string.friends_offer_joined_fmt, notice.teamName)
    }
    FriendsBanner(text, notice, onDismiss, modifier)
}

/**
 * The line of [FriendsNoticeBanner] for a one-shot that another tab owns, such as the Teams tab's "invitation sent" (D26). It stays for a few
 * seconds from the moment [key] changes; a tap dismisses it.
 */
@Composable
internal fun FriendsBanner(text: String, key: Any, onDismiss: () -> Unit, modifier: Modifier = Modifier) {
    LaunchedEffect(key) { delay(NOTICE_MS); onDismiss() }
    Surface(
        onClick = onDismiss, modifier = modifier.padding(16.dp).testTag(FriendsTags.NOTICE),
        shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.inverseSurface, shadowElevation = 4.dp,
    ) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.inverseOnSurface)
        }
    }
}

/**
 * A live offer's time left as the platform reads it in the future (D27): "in 3 days", "in 5 hours", the numeric form of the language's relative-time
 * text, never "tomorrow". The words come from ICU, so only the UI layer calls this.
 */
internal fun offerExpiryText(expiry: OfferExpiry.Live, locale: Locale): String = RelativeDateTimeFormatter.getInstance(locale).format(
    expiry.value.toDouble(), RelativeDateTimeFormatter.Direction.NEXT,
    if (expiry.unit == OfferUnit.Hour) RelativeDateTimeFormatter.RelativeUnit.HOURS else RelativeDateTimeFormatter.RelativeUnit.DAYS,
)

/** Names as a list in the person's language ("Ann, Ben and Cy"; "甲、乙和丙"). */
internal fun friendNamesText(names: List<String>, locale: Locale): String = ListFormatter.getInstance(locale).format(names)

private const val NOTICE_MS = 5_000L
