package com.lingxi.code.friends

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasAnyDescendant
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isPopup
import androidx.compose.ui.test.isToggleable
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.R
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.FriendsRequest
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.TabHost
import com.lingxi.code.teams.TeamLastMessage
import com.lingxi.code.testing.finishComposeFixture
import com.lingxi.code.theme.LingXiTheme
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Friends screens on a device (docs/FRIENDS-CLIENTS.md D3 to D8, D11, D12): the list, Add friend, the friend-link dialog, Profile
 * settings and Blocked people are driven with hand-made state and callbacks, so no Firebase project or account is needed. The signed-out
 * gate of the tab runs against the real default app and is skipped when someone is signed in on the device. The rules behind the
 * screens (order, filter, unread, badge, query kinds, relation actions, errors) run on the JVM in `FriendsLogicTest` against the shared
 * vectors; the workspace and the lookup flow in `FriendsWorkspaceTest` and `FriendLookupTest`.
 *
 * The content is laid out at a fixed dp size under a small fixed density, so every row is composed whatever the device is.
 */
@RunWith(AndroidJUnit4::class)
class FriendsScreensUiTest {
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @get:Rule
    val rule = createAndroidComposeRule<ComponentActivity>()

    private val events = mutableListOf<String>()
    private val lookups = mutableListOf<FriendProof>()
    private val notes = mutableListOf<String>()
    private val forms = mutableListOf<ProfileForm>()
    private var state by mutableStateOf(FriendsState())
    private var lookup by mutableStateOf(LookupState())

    private val uid = "u1"
    private val base = 1_793_606_400_000L
    private val code = "c".repeat(32)

    // --- Fixtures ----------------------------------------------------------------------------------------------------

    private fun friend(
        id: String, name: String, handle: String? = null, lastSeq: Long = 0, readSeq: Long = 0, at: Long? = null, preview: String = "hi", author: String = "u9",
        recalled: Boolean = false,
    ) = FriendCard(
        id, FriendState.Friends, name, handle, null, 0, PairRef("f_$id", 1), lastSeq, readSeq,
        at?.let { TeamLastMessage("m$id", maxOf(lastSeq, 1), it, "user", author, null, preview, recalled) },
    )

    private fun request(id: String, kind: FriendState, name: String, handle: String? = null, note: String? = null, since: Long = base) =
        FriendCard(id, kind, name, handle, note, since)

    private fun profile(name: String = "Me", handle: String? = "me_1", linkOn: Boolean = true, byHandle: Boolean = true, byEmail: Boolean = true) =
        FriendProfile(uid, name, handle, byHandle, byEmail, if (linkOn) FriendCode(code, "https://lingxios.net/open/friend/add?code=$code") else null)

    private fun loaded(vararg cards: FriendCard, profile: FriendProfile? = profile()) =
        FriendsState(uid = uid, cards = cards.toList(), cardsLoaded = true, profile = profile)

    private fun found(relation: FriendRelation, proof: FriendProof = FriendProof.Handle("ann"), name: String = "Ann", handle: String? = "ann") =
        LookupState(proof = proof, result = LookupResult("u2", name, handle, relation))

    // --- Hosting -----------------------------------------------------------------------------------------------------

    private fun show(height: Dp = 1600.dp, content: @Composable () -> Unit) {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalDensity provides Density(density = 0.5f, fontScale = 1f)) {
                    Box(Modifier.requiredSize(400.dp, height)) { content() }
                }
            }
        }
        rule.waitForIdle()
    }

    private fun showList(initial: FriendsState, open: String = "", height: Dp = 1600.dp) {
        state = initial
        show(height) {
            FriendsListScreen(
                state = state, uid = uid, openChatUid = open, listState = rememberLazyListState(), scrollToRequests = 0,
                onOpenChat = { events += "chat:$it" }, onAdd = { events += "add" }, onProfile = { events += "profile" }, onBlocked = { events += "blocked" },
                onRetry = { events += "retry" }, onRetryProfile = { events += "retryProfile" },
                onAccept = { events += "accept:$it" }, onDecline = { events += "decline:$it" }, onCancel = { events += "cancel:$it" },
            )
        }
    }

    private fun showAdd(initial: FriendsState = loaded(), lookupState: LookupState = LookupState(), showBack: Boolean = true) {
        state = initial
        lookup = lookupState
        show {
            AddFriendScreen(
                state = state, lookup = lookup, showBack = showBack, onClose = { events += "close" }, onOpenChat = { events += "chat:$it" },
                onLoadProfile = { events += "load" }, onRotateCode = { events += "rotate" }, onRevokeCode = { events += "revoke" },
                onLookup = { lookups += it }, onConfirm = { notes += it }, onClear = { events += "clear" },
            )
        }
    }

    private fun showDialog(initial: LookupState) {
        lookup = initial
        show {
            FriendLinkDialog(
                lookup = lookup, onConfirm = { events += "confirm" }, onRetry = { events += "retry" }, onOpenChat = { events += "chat:$it" },
                onDismiss = { events += "dismiss" },
            )
        }
    }

    private fun showProfile(initial: FriendsState, showBack: Boolean = true) {
        state = initial
        show {
            FriendProfileScreen(
                state = state, showBack = showBack, onClose = { events += "close" }, onLoad = { events += "load" }, onSave = { forms += it },
            )
        }
    }

    private fun showBlocked(initial: FriendsState, showBack: Boolean = true) {
        state = initial
        show {
            BlockedPeopleScreen(
                state = state, showBack = showBack, onClose = { events += "close" }, onRetry = { events += "retry" }, onUnblock = { events += "unblock:$it" },
            )
        }
    }

    private fun text(id: Int, vararg args: Any): String = rule.activity.getString(id, *args)
    private fun top(tag: String) = rule.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot.top
    private fun inDialog(label: String) = hasText(label) and hasAnyAncestor(isDialog())
    private fun update(change: () -> Unit) {
        rule.runOnUiThread(change)
        rule.waitForIdle()
    }

    // --- The list (D3, D4, D5) --------------------------------------------------------------------------------------

    @Test fun requestsComeFirstThenFriendsByActivityThenName() {
        showList(
            loaded(
                request("i1", FriendState.Incoming, "Ivy", since = base - 1_000), request("i2", FriendState.Incoming, "Ian", since = base),
                request("o1", FriendState.Outgoing, "Oz"),
                friend("f_cy", "Cy"), friend("f_dee", "dee"), friend("f_bob", "Bob", at = base, lastSeq = 1), friend("f_ann", "Ann", at = base + 3_600_000, lastSeq = 1),
            ),
        )
        val order = listOf(
            FriendsTags.REQUESTS, FriendsTags.row("i2"), FriendsTags.row("i1"), FriendsTags.row("o1"),
            FriendsTags.row("f_ann"), FriendsTags.row("f_bob"), FriendsTags.row("f_cy"), FriendsTags.row("f_dee"),
        )
        order.forEach { rule.onNodeWithTag(it).assertIsDisplayed() }
        val tops = order.map(::top)
        assertEquals("rows are not in the expected order: $tops", tops.sorted(), tops)
        rule.onNodeWithText(text(R.string.friends_requests_received)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_requests_sent)).assertIsDisplayed()
    }

    @Test fun aFriendRowShowsTheNameThePreviewAndTheUnreadCountCappedAt99() {
        showList(
            loaded(
                friend("a", "Ann", at = base, lastSeq = 9, readSeq = 4, preview = "see you"),
                friend("b", "Bob", at = base - 1, lastSeq = 250, readSeq = 0, preview = "lots"),
                friend("c", "Cy", at = base - 2, lastSeq = 3, readSeq = 3, preview = "read"),
            ),
        )
        rule.onNodeWithTag(FriendsTags.row("a")).assert(hasText("Ann") and hasText("see you") and hasText("5"))
        rule.onNodeWithTag(FriendsTags.row("b")).assert(hasText("99+"))
        rule.onNodeWithTag(FriendsTags.row("c")).assert(hasText("read") and hasText("3").not())
    }

    @Test fun theLinePreviewIsTheTeamsPreviewAndAFriendWithoutMessagesSaysSo() {
        showList(
            loaded(
                friend("a", "Ann"),
                friend("b", "Bob", at = base - 1, lastSeq = 1, author = uid, preview = "mine"),
                friend("c", "Cy", at = base - 2, lastSeq = 1, preview = "", recalled = true),
                friend("d", "Dee", at = base - 3, lastSeq = 1, preview = ""),
            ),
        )
        rule.onNodeWithTag(FriendsTags.row("a")).assert(hasText(text(R.string.teams_preview_none)))
        rule.onNodeWithTag(FriendsTags.row("b")).assert(hasText(text(R.string.teams_preview_you), substring = true) and hasText("mine", substring = true))
        rule.onNodeWithTag(FriendsTags.row("c")).assert(hasText(text(R.string.teams_message_recalled)))
        rule.onNodeWithTag(FriendsTags.row("d")).assert(hasText(text(R.string.teams_preview_non_text)))
    }

    @Test fun theChatInFrontCountsAsReadAndEveryOtherFriendKeepsItsUnread() {
        showList(
            loaded(friend("a", "Ann", at = base, lastSeq = 9, readSeq = 4), friend("b", "Bob", at = base - 1, lastSeq = 7, readSeq = 4)).copy(openChatUid = "a"),
            open = "a",
        )
        rule.onNodeWithTag(FriendsTags.row("a")).assert(hasText("5").not())
        rule.onNodeWithTag(FriendsTags.row("b")).assert(hasText("3"))
    }

    @Test fun tappingARowOpensThatFriendsChat() {
        showList(loaded(friend("a", "Ann"), friend("b", "Bob")))
        rule.onNodeWithTag(FriendsTags.row("b")).performClick()
        assertEquals(listOf("chat:b"), events)
    }

    // --- Requests (D4) ---------------------------------------------------------------------------------------------

    @Test fun aReceivedRequestShowsItsNoteAndAcceptAndDeclineAnswerIt() {
        showList(loaded(request("i1", FriendState.Incoming, "Ivy", handle = "ivy_1", note = "from the book club")))
        rule.onNodeWithTag(FriendsTags.row("i1")).assert(
            hasAnyDescendant(hasText("Ivy")) and hasAnyDescendant(hasText("@ivy_1")) and hasAnyDescendant(hasText("from the book club")),
        )
        rule.onNodeWithTag(FriendsTags.cancel("i1")).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.accept("i1")).performClick()
        rule.onNodeWithTag(FriendsTags.decline("i1")).performClick()
        assertEquals(listOf("accept:i1", "decline:i1"), events)
    }

    @Test fun aSentRequestSaysSoAndCancelWithdrawsIt() {
        showList(loaded(request("o1", FriendState.Outgoing, "Oz")))
        rule.onNodeWithTag(FriendsTags.row("o1")).assert(hasAnyDescendant(hasText(text(R.string.friends_request_sent))))
        rule.onNodeWithTag(FriendsTags.accept("o1")).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.cancel("o1")).performClick()
        assertEquals(listOf("cancel:o1"), events)
    }

    @Test fun aRowWithACallInFlightIsDisabledAndAFailedOneShowsItsError() {
        showList(
            loaded(request("i1", FriendState.Incoming, "Ivy"), request("i2", FriendState.Incoming, "Ian", since = base - 1))
                .copy(rowOps = mapOf("i1" to FriendsOp(busy = true), "i2" to FriendsOp(error = FriendsError.RateLimited))),
        )
        rule.onNodeWithTag(FriendsTags.accept("i1")).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.decline("i1")).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.accept("i2")).assertIsEnabled()
        rule.onNodeWithText(text(R.string.friends_error_rate_limited)).assertIsDisplayed()
    }

    @Test fun aNoteIsAtMostTwoLines() {
        val long = (1..60).joinToString(" ") { "word$it" }
        showList(
            loaded(
                request("i1", FriendState.Incoming, "Ivy", note = long),
                request("i2", FriendState.Incoming, "Ian", note = "hi", since = base - 1),
            ),
        )
        val tall = rule.onNode(hasText(long)).fetchSemanticsNode().boundsInRoot.height
        val short = rule.onNode(hasText("hi")).fetchSemanticsNode().boundsInRoot.height
        assertTrue("a long note is $tall px high, a one-line note $short px", tall <= short * 2 + 1f)
    }

    @Test fun aGroupPastTwentyRowsEndsWithShowAllWhichExpandsIt() {
        val many = (1..23).map { request("i%02d".format(it), FriendState.Incoming, "P$it", since = base - it) }
        showList(loaded(*many.toTypedArray()), height = 8000.dp)
        val showAll = text(R.string.friends_show_all_fmt, 23)

        (1..20).forEach { rule.onNodeWithTag(FriendsTags.row("i%02d".format(it))).assertExists() }
        rule.onNodeWithTag(FriendsTags.row("i21")).assertDoesNotExist()
        rule.onNodeWithText(showAll).performClick()
        rule.waitForIdle()

        rule.onNodeWithTag(FriendsTags.row("i23")).assertExists()
        rule.onNodeWithText(showAll).assertDoesNotExist()
    }

    // --- Loading, empty and failed (D3) ----------------------------------------------------------------------------

    @Test fun loadingEmptyAndFailedAreThreeDifferentStates() {
        showList(FriendsState(uid = uid))
        rule.onNodeWithTag(FriendsTags.LOADING).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.EMPTY).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.LOAD_FAILED).assertDoesNotExist()

        update { state = FriendsState(uid = uid, cardsFailed = true) }
        rule.onNodeWithTag(FriendsTags.LOAD_FAILED).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.LOADING).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retry"), events)

        update { state = FriendsState(uid = uid, cardsLoaded = true) }
        rule.onNodeWithTag(FriendsTags.EMPTY).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_empty_title)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_add)).performClick()
        assertEquals(listOf("retry", "add"), events)
    }

    @Test fun aListenerThatFailsLaterKeepsTheListAndOffersRetryInline() {
        showList(loaded(friend("a", "Ann")).copy(cardsFailed = true))
        rule.onNodeWithTag(FriendsTags.row("a")).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_load_error)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retry"), events)
    }

    // --- The filter (D5) -------------------------------------------------------------------------------------------

    @Test fun theFilterNarrowsFriendsByNameOrUsernameAndLeavesTheRequestsAlone() {
        showList(
            loaded(
                request("i1", FriendState.Incoming, "Ivy"),
                friend("a", "Zoe", handle = "zoe_m"), friend("b", "Bob", handle = "bobby"), friend("c", "Cy"),
            ),
        )
        rule.onNodeWithTag(FriendsTags.SEARCH).performClick()
        rule.onNodeWithTag(FriendsTags.FILTER_FIELD).performTextInput("ZOE")
        rule.waitForIdle()
        rule.onNodeWithTag(FriendsTags.row("a")).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.row("b")).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.row("c")).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.row("i1")).assertIsDisplayed()

        rule.onNodeWithTag(FriendsTags.FILTER_FIELD).performTextReplacement("@bobby")
        rule.waitForIdle()
        rule.onNodeWithTag(FriendsTags.row("b")).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.row("a")).assertDoesNotExist()

        rule.onNodeWithTag(FriendsTags.FILTER_FIELD).performTextReplacement("nobody at all")
        rule.waitForIdle()
        rule.onNodeWithTag(FriendsTags.FILTER_EMPTY).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.row("i1")).assertIsDisplayed()

        rule.onNodeWithContentDescription(text(R.string.common_close)).performClick()
        rule.waitForIdle()
        listOf("a", "b", "c", "i1").forEach { rule.onNodeWithTag(FriendsTags.row(it)).assertIsDisplayed() }
        rule.onNodeWithTag(FriendsTags.FILTER_EMPTY).assertDoesNotExist()
    }

    // --- The name banner and the header (D3) -----------------------------------------------------------------------

    @Test fun theFallbackNameShowsTheBannerThatOpensTheProfile() {
        showList(loaded(friend("a", "Ann"), profile = profile(name = FriendsLogic.FALLBACK_NAME)))
        rule.onNodeWithTag(FriendsTags.NAME_BANNER).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_name_prompt)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_name_prompt_action)).performClick()
        assertEquals(listOf("profile"), events)
    }

    @Test fun aChosenNameOrAnUnknownProfileShowsNoBanner() {
        showList(loaded(friend("a", "Ann"), profile = profile(name = "Mia")))
        rule.onNodeWithTag(FriendsTags.NAME_BANNER).assertDoesNotExist()
        update { state = loaded(friend("a", "Ann"), profile = null) }
        rule.onNodeWithTag(FriendsTags.NAME_BANNER).assertDoesNotExist()
    }

    @Test fun aProfileThatFailedToLoadShowsAnInlineRetry() {
        showList(loaded(friend("a", "Ann"), profile = null).copy(profileOp = FriendsOp(error = FriendsError.Network)))
        rule.onNodeWithTag(FriendsTags.PROFILE_ERROR).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
        rule.onNode(hasText(text(R.string.common_retry)) and hasAnyAncestor(hasTestTag(FriendsTags.PROFILE_ERROR))).performClick()
        assertEquals(listOf("retryProfile"), events)
    }

    @Test fun theHeaderOffersAddAndAMenuWithExactlyProfileSettingsAndBlockedPeople() {
        showList(loaded(friend("a", "Ann")))
        rule.onNodeWithText(text(R.string.friends_title)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.ADD).performClick()
        assertEquals(listOf("add"), events)

        rule.onNodeWithTag(FriendsTags.MORE).performClick()
        rule.onNodeWithText(text(R.string.friends_profile_title)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_blocked_title)).assertIsDisplayed()
        rule.onAllNodes(hasClickAction() and hasAnyAncestor(isPopup())).assertCountEquals(2)
        rule.onNodeWithText(text(R.string.friends_profile_title)).performClick()
        rule.waitForIdle()
        assertEquals(listOf("add", "profile"), events)

        rule.onNodeWithTag(FriendsTags.MORE).performClick()
        rule.onNodeWithText(text(R.string.friends_blocked_title)).performClick()
        rule.waitForIdle()
        assertEquals(listOf("add", "profile", "blocked"), events)
    }

    // --- Add friend (D6, D7) ---------------------------------------------------------------------------------------

    @Test fun myLinkCardShowsTheLinkItsFourActionsAndTheDisclosureBelowIt() {
        showAdd()
        assertTrue("the profile is asked for on every open: $events", "load" in events)
        rule.onNodeWithTag(FriendsTags.LINK_CARD).assertIsDisplayed()
        rule.onNodeWithText("https://lingxios.net/open/friend/add?code=$code").assertIsDisplayed()
        listOf(FriendsTags.LINK_COPY, FriendsTags.LINK_SHARE, FriendsTags.LINK_RESET, FriendsTags.LINK_OFF).forEach { rule.onNodeWithTag(it).assertIsDisplayed() }
        rule.onNodeWithTag(FriendsTags.LINK_CREATE).assertDoesNotExist()

        val card = rule.onNodeWithTag(FriendsTags.LINK_CARD).fetchSemanticsNode().boundsInRoot
        val disclosure = rule.onNodeWithText(text(R.string.friends_link_disclosure)).fetchSemanticsNode().boundsInRoot
        assertTrue("the disclosure (top ${disclosure.top}) sits under the card (bottom ${card.bottom})", disclosure.top >= card.bottom - 1f)
    }

    @Test fun copyingTheLinkSaysSoInline() {
        showAdd()
        rule.onNodeWithText(text(R.string.friends_link_copied)).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.LINK_COPY).performClick()
        rule.onNodeWithText(text(R.string.friends_link_copied)).assertIsDisplayed()
    }

    @Test fun resetAndTurnOffAskBeforeTheyActAndCancelChangesNothing() {
        showAdd()
        rule.onNodeWithTag(FriendsTags.LINK_RESET).performClick()
        rule.onNodeWithText(text(R.string.friends_link_reset_confirm)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.common_cancel))).performClick()
        rule.waitForIdle()
        assertTrue("nothing was sent: $events", "rotate" !in events && "revoke" !in events)

        rule.onNodeWithTag(FriendsTags.LINK_RESET).performClick()
        rule.onNode(inDialog(text(R.string.friends_link_reset))).performClick()
        rule.waitForIdle()
        assertEquals(1, events.count { it == "rotate" })

        rule.onNodeWithTag(FriendsTags.LINK_OFF).performClick()
        rule.onNodeWithText(text(R.string.friends_link_off_confirm)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.friends_link_off))).performClick()
        rule.waitForIdle()
        assertEquals(1, events.count { it == "revoke" })
    }

    @Test fun withoutALinkTheCardOffersToCreateOne() {
        showAdd(loaded(profile = profile(linkOn = false)))
        rule.onNodeWithText(text(R.string.friends_link_is_off)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.LINK_COPY).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.LINK_CREATE).performClick()
        assertEquals(1, events.count { it == "rotate" })

        update { state = state.copy(linkOp = FriendsOp(busy = true)) }
        rule.onNodeWithTag(FriendsTags.LINK_CREATE).assertIsNotEnabled()
    }

    @Test fun aRotateThatFailedShowsItsErrorInTheCard() {
        showAdd(loaded().copy(linkOp = FriendsOp(error = FriendsError.Network)))
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.LINK_COPY).assertIsDisplayed()
    }

    @Test fun theLinkCardWaitsForTheProfileAndOffersRetryWhenItFails() {
        showAdd(FriendsState(uid = uid))
        rule.onNodeWithTag(FriendsTags.LINK_COPY).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.PROFILE_ERROR).assertDoesNotExist()

        update { state = FriendsState(uid = uid, profileOp = FriendsOp(error = FriendsError.Network)) }
        rule.onNodeWithTag(FriendsTags.PROFILE_ERROR).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(2, events.count { it == "load" })
    }

    @Test fun backOnAPhoneAndCloseBesideTheListBothLeaveTheScreen() {
        showAdd(showBack = true)
        rule.onNodeWithTag(FriendsTags.BACK).assert(hasContentDescription(text(R.string.remote_back)))
        rule.onNodeWithTag(FriendsTags.BACK).performClick()
        assertEquals(1, events.count { it == "close" })
    }

    @Test fun besideTheListTheHeaderOffersCloseInsteadOfBack() {
        showAdd(showBack = false)
        rule.onNodeWithTag(FriendsTags.BACK).assert(hasContentDescription(text(R.string.common_close)))
    }

    @Test fun oneFieldTakesAUsernameAnEmailAddressOrAFriendLinkAndNothingIsLookedUpWhileTyping() {
        showAdd()
        rule.onNodeWithTag(FriendsTags.SEARCH_ACTION).assertIsNotEnabled()

        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performTextInput("@ann_1")
        rule.waitForIdle()
        assertTrue("typing looks nothing up: $lookups", lookups.isEmpty())
        assertTrue("editing the field drops the card: $events", "clear" in events)
        rule.onNodeWithTag(FriendsTags.SEARCH_ACTION).assertIsEnabled().performClick()

        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performTextReplacement("Ann@Example.com")
        rule.onNodeWithTag(FriendsTags.SEARCH_ACTION).performClick()

        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performTextReplacement("https://lingxios.net/open/friend/add?code=$code")
        rule.onNodeWithTag(FriendsTags.SEARCH_ACTION).performClick()

        assertEquals(listOf(FriendProof.Handle("ann_1"), FriendProof.Email("ann@example.com"), FriendProof.Code(code)), lookups)
    }

    @Test fun theKeyboardsSearchKeyLooksUpLikeTheButton() {
        showAdd()
        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performTextInput("ann_1")
        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performImeAction()
        assertEquals(listOf<FriendProof>(FriendProof.Handle("ann_1")), lookups)
    }

    @Test fun textThatIsNoneOfTheThreeKindsShowsTheHintAndCannotBeLookedUp() {
        showAdd()
        rule.onNodeWithText(text(R.string.friends_search_hint)).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).performTextInput("not a person!")
        rule.waitForIdle()
        rule.onNodeWithText(text(R.string.friends_search_hint)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.SEARCH_ACTION).assertIsNotEnabled()
    }

    // --- The result card (D6) --------------------------------------------------------------------------------------

    @Test fun aStrangerOffersAnOptionalNoteAndAddSendsIt() {
        showAdd(lookupState = found(FriendRelation.None))
        rule.onNodeWithTag(FriendsTags.RESULT).assertIsDisplayed()
        rule.onNodeWithText("Ann").assertIsDisplayed()
        rule.onNodeWithText("@ann").assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).performTextInput("from the book club")
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assertIsEnabled().performClick()
        assertEquals(listOf("from the book club"), notes)
    }

    @Test fun theNoteStopsAtTwoHundredCharacters() {
        showAdd(lookupState = found(FriendRelation.None))
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).performTextInput("x".repeat(FriendsLogic.NOTE_MAX))
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).performTextInput("y")
        rule.waitForIdle()
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assert(hasText("x".repeat(FriendsLogic.NOTE_MAX)))
    }

    @Test fun aPersonFoundByFriendLinkHasNoNoteField() {
        showAdd(lookupState = found(FriendRelation.None, proof = FriendProof.Code(code)))
        rule.onNodeWithTag(FriendsTags.RESULT).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).performClick()
        assertEquals(listOf(""), notes)
    }

    @Test fun someoneWhoAskedFirstOffersAcceptWithNoNote() {
        showAdd(lookupState = found(FriendRelation.Incoming))
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assert(hasText(text(R.string.friends_request_accept)))
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).performClick()
        assertEquals(listOf(""), notes)
    }

    @Test fun aRequestAlreadySentShowsADisabledRequestSent() {
        showAdd(lookupState = found(FriendRelation.Outgoing))
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assertIsNotEnabled().assert(hasText(text(R.string.friends_request_sent)))
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assertDoesNotExist()
    }

    @Test fun aFriendShowsFriendsWithOpenChat() {
        showAdd(lookupState = found(FriendRelation.Friends))
        rule.onNodeWithText(text(R.string.friends_relation_friends)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assert(hasText(text(R.string.friends_open_chat))).performClick()
        assertEquals(listOf("chat:u2"), events.filter { it.startsWith("chat:") })
    }

    @Test fun afterARequestWentOutTheCardSaysSoAndOffersNothingMore() {
        showAdd(lookupState = found(FriendRelation.None).copy(outcome = FriendOutcome.RequestSent("Ann")))
        rule.onNodeWithText(text(R.string.friends_request_sent_to_fmt, "Ann")).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assertDoesNotExist()
    }

    @Test fun aNewFriendshipClearsTheCardAndOpensTheChat() {
        showAdd(lookupState = found(FriendRelation.None))
        update { lookup = lookup.copy(outcome = FriendOutcome.NowFriends("u2", "Ann")) }
        assertTrue("the card is dropped: $events", "clear" in events)
        assertTrue("the chat opens: $events", "chat:u2" in events)
    }

    @Test fun whileConfirmingTheFieldsAndTheActionAreLocked() {
        showAdd(lookupState = found(FriendRelation.None).copy(confirming = true))
        rule.onNodeWithTag(FriendsTags.RESULT_ACTION).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.RESULT_NOTE).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.SEARCH_INPUT).assertIsNotEnabled()
    }

    @Test fun aFailedLookupShowsItsErrorUnderTheFieldAndAFailedConfirmationInsideTheCard() {
        showAdd(lookupState = LookupState(proof = FriendProof.Handle("nobody"), error = FriendsError.NotFound))
        rule.onNodeWithTag(FriendsTags.SEARCH_ERROR).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_error_not_found)).assertIsDisplayed()

        update { lookup = found(FriendRelation.None).copy(error = FriendsError.FriendLimit) }
        rule.onNodeWithTag(FriendsTags.SEARCH_ERROR).assertDoesNotExist()
        rule.onNodeWithTag(FriendsTags.RESULT).assert(hasAnyDescendant(hasText(text(R.string.friends_error_friend_limit))))
    }

    // --- Opening a friend link (D8) --------------------------------------------------------------------------------

    @Test fun aFriendLinkIsLookedUpBehindASpinnerThatCanBeCancelled() {
        showDialog(LookupState(proof = FriendProof.Code(code), looking = true))
        rule.onNodeWithTag(FriendsTags.LINK_DIALOG).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.common_cancel))).performClick()
        assertEquals(listOf("dismiss"), events)
    }

    @Test fun theLinkDialogNamesThePersonAndOnlyAddingRedeemsIt() {
        showDialog(found(FriendRelation.None, proof = FriendProof.Code(code)))
        rule.onNodeWithText(text(R.string.friends_confirm_title_fmt, "Ann")).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_confirm_link_body)).assertIsDisplayed()
        assertTrue("nothing is confirmed by opening the link: $events", events.isEmpty())
        rule.onNode(inDialog(text(R.string.friends_add))).performClick()
        assertEquals(listOf("confirm"), events)
        rule.onNode(inDialog(text(R.string.common_cancel))).performClick()
        assertEquals(listOf("confirm", "dismiss"), events)
    }

    @Test fun aLinkOfSomeoneAlreadyAFriendOpensTheirChat() {
        showDialog(found(FriendRelation.Friends, proof = FriendProof.Code(code)))
        rule.onNodeWithText(text(R.string.friends_relation_friends)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.friends_open_chat))).performClick()
        assertEquals(listOf("dismiss", "chat:u2"), events)
    }

    @Test fun aLinkThatFindsNobodyIsNoLongerValid() {
        showDialog(LookupState(proof = FriendProof.Code(code), error = FriendsError.NotFound))
        rule.onNodeWithText(text(R.string.friends_link_invalid)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.common_close))).performClick()
        assertEquals(listOf("dismiss"), events)
    }

    @Test fun aLinkThatCouldNotBeReachedOffersRetryWithTheSameLink() {
        showDialog(LookupState(proof = FriendProof.Code(code), error = FriendsError.Network))
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.common_retry))).performClick()
        rule.onNode(inDialog(text(R.string.common_cancel))).performClick()
        assertEquals(listOf("retry", "dismiss"), events)
    }

    @Test fun whileTheLinkIsBeingRedeemedAddIsLockedAndAFailureShowsInsideTheDialog() {
        showDialog(found(FriendRelation.None, proof = FriendProof.Code(code)).copy(confirming = true))
        rule.onNode(inDialog(text(R.string.common_cancel))).assertIsNotEnabled()

        update { lookup = found(FriendRelation.None, proof = FriendProof.Code(code)).copy(error = FriendsError.Network) }
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
        rule.onNode(inDialog(text(R.string.friends_add))).assertIsEnabled()
    }

    // --- Profile settings (D11) ------------------------------------------------------------------------------------

    @Test fun theProfileIsAskedForOnOpenAndShowsAProgressUntilItArrives() {
        showProfile(FriendsState(uid = uid))
        assertTrue("the profile is asked for on every open: $events", "load" in events)
        rule.onNodeWithTag(FriendsTags.LOADING).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).assertDoesNotExist()

        update { state = loaded(profile = profile(name = "Mia", handle = "mia_9")) }
        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).assert(hasText("Mia"))
        rule.onNodeWithTag(FriendsTags.PROFILE_HANDLE).assert(hasText("mia_9"))
    }

    @Test fun aProfileThatCannotBeLoadedOffersRetry() {
        showProfile(FriendsState(uid = uid, profileOp = FriendsOp(error = FriendsError.Network)))
        rule.onNodeWithTag(FriendsTags.PROFILE_ERROR).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(2, events.count { it == "load" })
    }

    @Test fun saveStaysDisabledUntilSomethingChangesAndThenSendsOnlyTheForm() {
        val current = profile()
        showProfile(loaded(profile = current))
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsNotEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).performTextReplacement("Mia")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsEnabled().performClick()
        val form = forms.single()
        assertEquals(ProfileForm("Mia", "me_1", true, true), form)
        assertEquals(mapOf<String, Any>("displayName" to "Mia"), form.changes(current))
    }

    @Test fun anEmptyNameOrAMalformedUsernameDisablesSaveButAnEmptyUsernameDoesNot() {
        showProfile(loaded(profile = profile(handle = null)))
        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).performTextReplacement("")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsNotEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).performTextReplacement("Mia")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_HANDLE).performTextReplacement("ab")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsNotEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_HANDLE).performTextReplacement("mia_9")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_HANDLE).performTextReplacement("")
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsEnabled()

        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).performTextReplacement("n".repeat(FriendsLogic.DISPLAY_NAME_MAX + 1))
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsNotEnabled()
    }

    @Test fun theTwoSwitchesChangeWhoCanFindMe() {
        val current = profile()
        showProfile(loaded(profile = current))
        rule.onAllNodes(isToggleable()).assertCountEquals(2)
        rule.onAllNodes(isToggleable())[0].performClick()
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsEnabled().performClick()
        assertEquals(mapOf<String, Any>("findByHandle" to false), forms.single().changes(current))
    }

    @Test fun aTakenUsernameIsShownUnderTheFieldInsteadOfTheHint() {
        showProfile(loaded().copy(saveOp = FriendsOp(error = FriendsError.HandleTaken)))
        rule.onNodeWithText(text(R.string.friends_error_handle_taken)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_profile_handle_hint)).assertDoesNotExist()
    }

    @Test fun anotherSaveErrorShowsBelowTheSwitchesAndKeepsTheHint() {
        showProfile(loaded().copy(saveOp = FriendsOp(error = FriendsError.Network)))
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_profile_handle_hint)).assertIsDisplayed()
    }

    @Test fun whileSavingTheFormIsLocked() {
        showProfile(loaded().copy(saveOp = FriendsOp(busy = true)))
        rule.onNodeWithTag(FriendsTags.PROFILE_NAME).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.PROFILE_HANDLE).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.PROFILE_SAVE).assertIsNotEnabled()
    }

    // --- Blocked people (D12) --------------------------------------------------------------------------------------

    @Test fun blockedPeopleAreListedNewestFirstAndUnblockNeedsNoConfirmation() {
        showBlocked(
            FriendsState(
                uid = uid, blocksLoaded = true,
                blocks = listOf(
                    BlockEntry("b1", base - 2_000, "Old", "old_h"), BlockEntry("b2", base, "New", null), BlockEntry("b3", base - 1_000, "", null),
                ),
            ),
        )
        val order = listOf(FriendsTags.unblock("b2"), FriendsTags.unblock("b3"), FriendsTags.unblock("b1"))
        order.forEach { rule.onNodeWithTag(it).assertIsDisplayed() }
        val tops = order.map(::top)
        assertEquals("rows are not newest first: $tops", tops.sorted(), tops)
        rule.onNodeWithText("@old_h").assertIsDisplayed()
        rule.onNodeWithText(text(R.string.teams_member)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_unblock_footer)).assertIsDisplayed()

        rule.onNodeWithTag(FriendsTags.unblock("b3")).performClick()
        assertEquals(listOf("unblock:b3"), events)
    }

    @Test fun anUnblockInFlightIsDisabledAndAFailedOneShowsItsError() {
        showBlocked(
            FriendsState(
                uid = uid, blocksLoaded = true, blocks = listOf(BlockEntry("b1", base, "Old", null), BlockEntry("b2", base - 1, "Other", null)),
                rowOps = mapOf("b1" to FriendsOp(busy = true), "b2" to FriendsOp(error = FriendsError.Network)),
            ),
        )
        rule.onNodeWithTag(FriendsTags.unblock("b1")).assertIsNotEnabled()
        rule.onNodeWithTag(FriendsTags.unblock("b2")).assertIsEnabled()
        rule.onNodeWithText(text(R.string.friends_error_network)).assertIsDisplayed()
    }

    @Test fun blockedPeopleLoadingEmptyAndFailedAreThreeDifferentStates() {
        showBlocked(FriendsState(uid = uid))
        rule.onNodeWithTag(FriendsTags.LOADING).assertIsDisplayed()

        update { state = FriendsState(uid = uid, blocksFailed = true) }
        rule.onNodeWithTag(FriendsTags.LOAD_FAILED).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.common_retry)).performClick()
        assertEquals(listOf("retry"), events)

        update { state = FriendsState(uid = uid, blocksLoaded = true) }
        rule.onNodeWithTag(FriendsTags.BLOCKED_EMPTY).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_blocked_empty)).assertIsDisplayed()
    }

    @Test fun theBlockedPageHasItsTitleAndLeavesThroughBack() {
        showBlocked(FriendsState(uid = uid, blocksLoaded = true))
        rule.onNodeWithText(text(R.string.friends_blocked_title)).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.BACK).performClick()
        assertEquals(listOf("close"), events)
    }

    // --- The notice line (D4, D7, D16) -----------------------------------------------------------------------------

    @Test fun aNewFriendNoticeNamesThemAndATapDismissesIt() {
        show { FriendsNoticeBanner(FriendsNotice.NowFriends("Ann"), onDismiss = { events += "dismiss" }) }
        rule.onNodeWithTag(FriendsTags.NOTICE).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_now_friends_fmt, "Ann")).assertIsDisplayed()
        rule.onNodeWithTag(FriendsTags.NOTICE).performClick()
        assertTrue("a tap dismisses it: $events", "dismiss" in events)
    }

    @Test fun anUnavailableChatNoticeSaysSoAndDismissesItselfAfterAFewSeconds() {
        show { FriendsNoticeBanner(FriendsNotice.ChatUnavailable, onDismiss = { events += "dismiss" }) }
        rule.onNodeWithText(text(R.string.friends_chat_unavailable)).assertIsDisplayed()
        assertTrue("it stays for a while: $events", events.isEmpty())
        rule.waitUntil(timeoutMillis = 10_000) { "dismiss" in events }
    }

    // --- The tab against the real default app ------------------------------------------------------------------------

    private class RecordingHost : TabHost {
        val calls = mutableListOf<String>()
        override fun select(tab: AppTab) { calls += "select:${tab.key}" }
        override fun openSettings(route: String, origin: AppTab?) { calls += "settings:$route:${origin?.key}" }
        override fun reportDepth(tab: AppTab, depth: Int) { calls += "depth:${tab.key}:$depth" }
        override fun requestFriends(link: FriendLink) { calls += "friends:$link" }
    }

    private fun showTab(host: RecordingHost, request: FriendsRequest?) {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalTabHost provides host) {
                    FriendsTab(request = request, onRequestHandled = { events += "handled:$it" })
                }
            }
        }
        rule.waitForIdle()
    }

    @Test fun signedOutTheTabShowsTheGateAndSignInJumpsToAccountSettingsFromFriends() {
        assumeTrue("A user is signed in on this device", FirebaseAuth.getInstance().currentUser == null)
        val host = RecordingHost()
        showTab(host, null)
        rule.onNodeWithTag(FriendsTags.SIGN_IN).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.friends_signin)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.remote_sign_in)).performClick()
        assertTrue(host.calls.toString(), "settings:${SettingsRoutes.ACCOUNT}:friends" in host.calls)
    }

    @Test fun aFriendLinkWaitsForSignInInsteadOfBeingConsumed() {
        assumeTrue("A user is signed in on this device", FirebaseAuth.getInstance().currentUser == null)
        showTab(RecordingHost(), FriendsRequest(1, FriendLink.Add(code)))
        rule.onNodeWithTag(FriendsTags.SIGN_IN).assertIsDisplayed()
        assertTrue("the link was handled while signed out: $events", events.none { it.startsWith("handled") })
        rule.onNodeWithTag(FriendsTags.LINK_DIALOG).assertDoesNotExist()
    }
}
