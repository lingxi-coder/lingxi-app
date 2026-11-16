# Teams UI polish (U1 to U13)

Status: the UI contract for the Team, Friend and Bot-chat surfaces on four clients: **Desktop and Web** (one React
implementation in `packages/teams` plus host CSS), **iOS** (SwiftUI) and **Android** (Compose). The strings it needs are
already in the five `resources/translations/*.json` files (section 2); the client changes are not made yet. If a client and
this document disagree, fix the client; if the document is wrong, change it and the strings together.

Scope: "chat" means the Team, Friend and Bot-chat message views and the lists, detail pages and dialogs around them, not the
main AI chat. The native shells get visual and accessibility changes only; the hand-rolled panes stay.

How to read it: each decision is one rule, then the translation for the platforms where it differs.

| Client | Where the change lands |
| --- | --- |
| iOS | `apps/ios/native/Sources/{Teams,Friends,Bots,Remote,Conversation}` |
| Android | `apps/android/native/app/src/main/java/com/lingxi/code/{teams,friends,bots,remote,theme}` |
| Desktop and Web | `packages/teams` (`workspace.css`, components), Desktop theme `apps/electron/src/renderer/theme/teamsTheme.ts`, Web `apps/web/src/teams/teams-web.css` |

## 1. Decisions

**U1 Glossary.** One word per concept, in every client and every locale.

| Concept | en | zh-Hans | zh-Hant | ja | ko | Never |
| --- | --- | --- | --- | --- | --- | --- |
| Bot | Bot | Bot | Bot | Bot | Bot | assistant, 助手, "Assistants", "Team assistants" |
| A conversation with one Bot | Bot chats (the section) | Bot 对话 | Bot 對話 | Bot とのチャット | Bot 대화 | Bot DMs |
| Person to person | Direct message | 私信 | 私訊 | ダイレクトメッセージ | 개인 메시지 | 私聊 |
| Several people | Group | 群聊 | 群組 | グループ | 그룹 | |
| Team | Team | 团队 | 團隊 | チーム | 팀 | 群组 |
| The person who runs and shares a Bot | Maintainer | 维护者 | 維護者 | メンテナー | 메인테이너 | owner, 所有者 (for a Bot) |
| The person who created a team | Owner | 所有者 | 擁有者 | オーナー | 소유자 | maintainer |
| A team manager | Admin | 管理员 | 管理員 | 管理者 | 관리자 | |
| Ending a Bot's availability | Stop sharing | 停止共享 | 停止共享 | 共有を停止 | 공유 중지 | Pause, Unbind, Rebind, Disable |

- **Stop sharing, decided from code.** The Teams button "Pause sharing" and the Friends button "Stop sharing" send the same
  command, `pauseSharedBot` (`apps/firebase/src/shared-bots.mjs`, `pause()`): it sets `identity.state` to `paused`, raises the
  revision by one, and only the maintainer may send it (`MAINTAINER_REQUIRED`). Desktop sends the same command from its Bot
  directory (`packages/teams/BotDirectory.tsx`) and its main process then calls `host.stopBinding`
  (`apps/electron/src/main/teams/service.ts`); sharing the Bot again from Desktop restores it. The two names describe one effect, so there is one name, "Stop sharing"
  (`friends_share_bot_stop`), and one explanation (`teams_bot_stop_detail`). A paused Bot reads "Sharing stopped" (`teams_bot_disabled`).
- **Team term, decided from the catalog.** The plan proposed 群组 for a team. The catalog has no 群组 for a team: 团队 is used in 35
  `teams_*` and `friends_*` strings, and 群聊 (zh-Hant 群組) already means a group conversation. The team term stays 团队, 團隊,
  チーム, 팀. Chinese direct message is 私信 everywhere (four keys already say it; `teams_conversation_dm` said 私聊 and is fixed).
- **Maintainer in ja and ko.** The existing ja and ko values left the word in English. The transliterations メンテナー and
  메인테이너 keep the register of the English and Chinese words and stay distinct from 管理者 and 관리자 (Admin) and from
  オーナー and 소유자 (Owner of a team).
- **One verb per object:** recall a message (撤回), revoke an invite (撤销), remove a member or a friend, delete a team,
  forget a computer, stop sharing a Bot, block a person. Do not swap them.
- Sentence case, one ellipsis character, "LingXi" in en, ja and ko and 灵犀 / 靈犀 in the Chinese locales.

**U2 Avatars.** Five sizes; the shape tells people from Bots.

| Size | Used for |
| --- | --- |
| 20 | chips: a Bot in "Bots that will be asked", the team chip, inline mentions |
| 24 | mention rows, pickers (New conversation lists) |
| 32 | message author, conversation header |
| 40 | list rows: conversations, members, Bots, friends |
| 56 | profile and detail headers: team details, a Bot card, a friend's profile |

Drop the stray 28, 30, 36, 44, 48 and 72. Shapes: a person is a circle; a Bot is its AgentAvatar, keyed by the Bot's `avatarId`
(never by name or index), clipped to a continuous rounded rectangle with radius 0.24 x size; the `#` channel tile and the team
mark use the same 0.24 x size rounded rectangle.

| Client | Translation |
| --- | --- |
| iOS | `.clipShape(RoundedRectangle(cornerRadius: size * 0.24, style: .continuous))` on `AgentAvatar`; size from `@ScaledMetric(relativeTo: .body)`, capped at 1.5 x the base. |
| Android | `BotAvatar(avatarId, size)` clipped with `RoundedCornerShape(size * 0.24f)`; sizes are dp; initials are sized from the dp size (U7). Key `BotAvatar` by `avatarId` in every call site. |
| Desktop and Web | `border-radius: 24%` on Bot, channel and team avatars; Web renders the AgentAvatar art through `renderAvatar` instead of a letter tile. |

**U3 Own message.** In Team and Friend streams, a message written by the signed-in person shows the author as "You"
(`teams_you`) and its row gets a faint tint: the accent at 6 percent over the stream background. Layout and alignment do not change
(rows stay flat; there are no bubbles).
- The tint is background only: no border, no change of text colour. Body text keeps the primary text colour; time and "Edited"
  keep the muted colour.
- Consecutive messages of one author inside the 5-minute group window form one tinted block; "You" is on the group's first row.
- Bot replies are never tinted. Hover and selection layer on top of the tint.
- Assistive technology reads the author as "You".
- Contrast is computed, not assumed (Appendix D): body text on the tint is at least 12.9:1 on every client and theme, and the muted
  text of Desktop is 5.49:1 (light) and 7.23:1 (dark). The brief's pairs: Desktop light #1d1d1f on `#0066d6` at 6 percent over
  `#ffffff` (row `#f0f6fd`) is 15.44:1; Desktop dark #f5f5f7 on `#0a84ff` at 6 percent over `#1e1e20` (row `#1d242d`) is 14.35:1.
  Both are at least 4.5:1.

| Client | Translation |
| --- | --- |
| iOS | `.background(Color.accentColor.opacity(0.06))` on the message row. |
| Android | `Modifier.background(MaterialTheme.colorScheme.primary.copy(alpha = 0.06f))` on the row. |
| Desktop and Web | token `--tw-own: var(--teams-own, color-mix(in srgb, var(--tw-accent) 6%, transparent))`, defined with the other tokens on `.tw-scope, .tw-dialog`; the row uses `background: var(--tw-own)`. The accent already switches per scheme, so one definition covers light and dark. |

**U4 Run card.** One anatomy on every client: (1) state: glyph and label; (2) title: the Bot's name; (3) detail: one line;
(4) actions: the fewest that fit the state (see below). Applies to the Team task card (`TeamTaskCard` in `Teams/TeamBotViews.swift` and
`teams/TeamBotComponents.kt`) and `BotRun` in `packages/teams/BotRunCards.tsx`. The fixed-size `ConversationExecutionRunCard`
in `Conversation/ConversationExecutionViews.swift` belongs to the main chat: it only gets the type fix of U7.
- The state is never colour alone: the glyph and the label are always both there. The glyph is decorative (hidden from assistive
  technology); the label is the accessible state. The card is one accessibility element: "Bot, state, detail", with the action
  exposed as a button.
- Tone: only Completed uses the ok colour and only Failed the danger colour; every other state uses the muted tone.
- The detail line is the state's detail text, or for Waiting who the task waits for and when the wait expires, and for Needs
  checking what has to be confirmed (maintainer or not). The usage line (U5) follows for Completed always, and for any other state
  only when the run reports usage, so a cost is never hidden.
- The label reads "Cancelling…" (`teams_task_cancelling`) while a cancel is on its way, and "Waiting for approval"
  (`teams_task_waiting_approval`) when a Waiting task waits on a tool approval.
- Active states (Queued, Processing, Authorization required) offer "Refresh status" (`teams_run_refresh`); today the state
  label itself is the refresh button on iOS and Android, which hides the action. A finished run with a document offers "Open
  document" (`teams_artifact_open`). Failed, Expired, Interrupted, Cancelled and Access revoked have no button: the detail says what to do.
- Added with Teams M2: the requester or the maintainer can Cancel a Queued, Processing or Waiting task (`teams_task_cancel`);
  the recipient of a Bot's question gets Reply; Needs checking offers Review actions (maintainer) or View actions (requester).
  So Waiting can show Refresh, Reply and Cancel together; that is the one exception to a single action.
- Reduce motion: the Processing spinner becomes its static glyph.

| State | Label key | Detail key | SF Symbol | Material icon | CSS (inline SVG `Icon`) | Tone |
| --- | --- | --- | --- | --- | --- | --- |
| queued | `teams_run_queued` | `teams_run_queued_detail` | `clock` | `Schedule` | clock | muted |
| running | `teams_run_processing` | `teams_run_processing_detail` | `ProgressView` (reduce motion: `ellipsis.circle`) | `CircularProgressIndicator` (reduce motion: `HourglassEmpty`) | spinner (reduce motion: dots) | muted |
| completed | `chat_status_completed` | the usage line | `checkmark.circle.fill` | `CheckCircle` | check | ok |
| failed | `chat_status_failed` | `teams_run_failed_detail` | `exclamationmark.triangle.fill` | `ErrorOutline` | alert | danger |
| expired | `teams_run_expired` | `teams_run_expired_detail` | `clock.badge.xmark` | `TimerOff` | clock with slash | muted |
| interrupted | `teams_run_interrupted` | `teams_run_interrupted_detail` | `pause.circle` | `PauseCircle` | pause | muted |
| authorization_required | `teams_run_authorization_required` | `teams_run_authorization_required_detail` | `lock.circle` | `Lock` | lock | muted |
| revoked | `teams_run_revoked` | `teams_run_revoked_detail` | `nosign` | `Block` | ban | muted |
| waiting | `teams_task_waiting` | written by the card (who, expiry) | `ellipsis.bubble` | `Pending` | dots | muted |
| cancelled | `teams_task_cancelled` | Web: "This request was cancelled."; mobile: none | `xmark.circle` | `Cancel` | x in circle | muted |
| needs_reconciliation | `teams_task_needs_checking` | written by the card (maintainer or not) | `questionmark.circle` | `FactCheck` | help | muted |
| any other value | `chat_status_unknown` | none | `questionmark.circle` | `HelpOutline` | help | muted |

What the states mean, from `apps/firebase/src/shared-bots.mjs`: a queued task past its deadline reads Expired (the Bot's Desktop
did not pick it up in time); a running task past its deadline reads Interrupted; Authorization required means the result was
withheld because not every reader of the conversation may see it, so the person is told to ask the maintainer.

Tone tokens: iOS the system green and red for the glyph and `.secondary` for muted; Android the theme's existing online colour,
`colorScheme.error` and `onSurfaceVariant`; CSS `--tw-online`, `--tw-danger` and `--tw-muted`. On Desktop, Web and Android all three
tones pass 3:1 as graphics on the card surface in both themes (Appendix D). On iOS the system green is 2.22:1 on white in light mode
(red 3.55:1), so the glyph tint is never the only carrier: the label is text in the primary colour and says the state.

**U5 Usage line.** `<tokens> tokens` then ` · ` then the cost, on one line.
- Tokens are the total of input and output, in the platform's compact number format (`1.2K` in en; the formatter localizes it),
  through `teams_usage_tokens_total`. The split stays available as the accessibility value or tooltip through the existing
  `teams_usage_tokens`.
- Cost is the amount in the currency of the task, with at least 2 and at most 4 decimals ("$0.0012", not "0.001322"). A positive cost
  below 0.0001 reads "<" followed by the smallest amount. No `costMicros` reads "cost unknown" (`teams_usage_cost_unknown`); a cost
  without a currency shows the amount and "currency unknown" (`teams_usage_currency_unknown`). A cost is never invented.
- Status `missing` or no usage reads "Usage unavailable" (`teams_usage_missing`); `partial` puts "Partial usage"
  (`teams_usage_partial`) in front; `complete` has no prefix (`teams_usage_complete` is superseded).

| Client | Number and currency |
| --- | --- |
| iOS | `Int.formatted(.number.notation(.compactName))`; `Decimal.formatted(.currency(code:).precision(.fractionLength(2...4)))`. |
| Android | `android.icu.text.CompactDecimalFormat` (SHORT) for tokens; `NumberFormat.getCurrencyInstance` with `minimumFractionDigits = 2`, `maximumFractionDigits = 4`. |
| Desktop and Web | `Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 1 })`; `{ style: 'currency', currency, minimumFractionDigits: 2, maximumFractionDigits: 4 }`. |

**U6 Targets.** Controls are never smaller than the platform floor.

| Client | Floor | How |
| --- | --- | --- |
| iOS | 44 x 44 pt hit area | The visual size may stay smaller: `.frame(minWidth: 44, minHeight: 44)` then `.contentShape(Rectangle())`. No `.controlSize(.small)` on row actions (Accept, Decline, Join, Unblock, Message Bot, Stop sharing). The composer `@` and Send are 44 x 44; Discard is `.footnote` with a 44 pt hit area. |
| Android | 48 x 48 dp | `Modifier.minimumInteractiveComponentSize()` on icon buttons; `Modifier.heightIn(min = 48.dp)` on Bots chat rows, New chat and Show more rows, section headers, mention rows and the team chip. The uses stepper is two `IconButton`s labelled `teams_uses_decrease` and `teams_uses_increase`. |
| Desktop, pointer | 28 px | A glyph-only control in a dense row may be 20 px only if the 24 px circle around it touches no other target (the WCAG 2.5.8 spacing exception); WCAG 2.5.8 (AA) otherwise asks for 24 px. |
| Web, touch | 44 px | `@media (hover: none) and (pointer: coarse)` sets `min-height` and `min-width` to 44px for buttons, row actions and list rows (replaces the 28px of `workspace.css`). |

**U7 Type.** System faces only; text scales on every client.

| Client | Rule |
| --- | --- |
| iOS | Text styles only: `.body` message text, `.subheadline` secondary, `.footnote` run-card label and detail, `.caption` metadata; no `.system(size:)`. Icons and avatars scale with `@ScaledMetric(relativeTo:)`. The unread badge is `.caption2.bold()` (bold text needs 3:1 under the HIG; white on the accent measures 4.02:1 light and 3.65:1 dark) and keeps its `accessibilityLabel`. The team chip loses `maxWidth: 150` and truncates in the toolbar. |
| Android | `MaterialTheme.typography` roles only. Header rows are `heightIn(min = 56.dp)`, never a fixed height. Avatar initials are `size * 0.4f` taken as dp and converted with `LocalDensity`, so a larger font scale cannot push them out of the circle. `WaitingBadge` is `widthIn(min = 22.dp)` with padding. |
| Desktop and Web | Body 13px, secondary 12px, floor 11px; the `MemberHome` heading is 19px like `.tw-empty`. |

**U8 Confirmations.** Every destructive or consequential action asks once, with a title that names the thing, a body only when it
adds something, a verb for the button, and a Cancel that is always there (HIG Alerts: "Write a title that clearly and succinctly
describes the situation", "include a Cancel button"). The Cancel label is `common_cancel`, never `teams_cancel` (still English in
zh-Hant, ja and ko).

| Action | Title | Body | Confirm button | Style |
| --- | --- | --- | --- | --- |
| Leave team | `teams_confirm_leave_title %@` | `teams_confirm_leave_body` | `teams_confirm_leave_button` | destructive |
| Delete team (owner) | `teams_confirm_delete_title %@` | `teams_confirm_delete_body` | `teams_confirm_delete_button` | destructive |
| Remove member | `teams_confirm_remove_title %@` | `teams_confirm_remove_body` | `teams_confirm_remove_button` | destructive |
| Make admin | `teams_confirm_promote_title %@` | `teams_confirm_promote_body` | `teams_confirm_promote_button` | default |
| Make member | `teams_confirm_demote_title %@` | `teams_confirm_demote_body` | `teams_confirm_demote_button` | default |
| Revoke invite (new) | `teams_confirm_revoke_title` | `teams_confirm_revoke_body` | `teams_confirm_revoke_button` | destructive |
| Recall message | `teams_recall_confirm` | `teams_recall_confirm_detail` | `teams_message_recall` | destructive |
| Stop sharing a Bot (Team and Friend) | `teams_bot_stop_title %@` | `teams_bot_stop_detail` | `friends_share_bot_stop` | destructive |
| Remove friend | `friends_remove_title %@` | `friends_remove_body` | `friends_menu_remove` | destructive |
| Block | `friends_block_title %@` | `friends_block_body` | `friends_menu_block` | destructive |
| Reset friend link | `friends_link_reset_title` | `friends_link_reset_body` | `friends_link_reset` | destructive |
| Turn off friend link | `friends_link_off_title` | `friends_link_off_body` | `friends_link_off` | destructive |
| Forget computer (new) | `remote_forget_title` | `remote_forget_help` | `remote_forget_confirm` | destructive |

No confirmation, because the act is undoable or already explicit: Unblock, Decline a friend request, Cancel a sent request,
Dismiss a team offer, Discard a failed pending message (the text is still on screen and the button is the explicit act), Mark read,
Accept, Join. Today's "Confirm this membership change?" (`teams_confirm_change`) and a title-only "Leave this team?" are replaced.
The Stop sharing dialog is one dialog, with the Bot's name in the title, on Teams and Friends alike.

| Client | Translation |
| --- | --- |
| iOS | Keep each surface's container (`confirmationDialog` anchored on the button, or `alert`); title, message, a `Button(role: .destructive)` for destructive rows and `Button(role: .cancel)`. |
| Android | `AlertDialog` with `title`, `text`, a `confirmButton` `TextButton` in `colorScheme.error` when destructive, and a `dismissButton`; never a title-only dialog, never the word "Confirm". |
| Desktop and Web | `Dialog` with `role="alertdialog"`, `aria-labelledby` the title, `aria-describedby` the body, initial focus on Cancel (as today), a confirm button using `--tw-danger` when destructive, and Cancel always present (the non-friend Pause dialog of `BotDirectory.tsx` has none today). |

**U9 Empty states.** A blank screen says what is empty, why it matters if that is not obvious, and what to do next.

| Surface | Title | Body | Action |
| --- | --- | --- | --- |
| Teams list, no teams | `teams_empty_title` | `teams_empty` | `teams_create`, `teams_join` (8 pt / 8dp apart) |
| Conversations, none | `teams_no_conversations` | `teams_no_conversations_hint` | `teams_conversation_new` |
| A thread with no message | `teams_preview_none` | `teams_empty_thread_hint` | none (the composer is the action) |
| Search with no match | `teams_no_matches` | none | none |
| Team Bots, none | `teams_bots_none` | `teams_bots_share_hint` | none |
| Friends, none (list and Desktop/Web main area) | `friends_empty_title` | `friends_empty_body` | `friends_add` |
| A friend's Bots, none | `friends_bots_none` | `friends_bots_share_hint` | none |
| Blocked people, none | `friends_blocked_empty_title` | `friends_blocked_empty_body` | none |
| Bots tab, no remote Bots on the computer | `mobile_bots_none_title` | `mobile_bots_none_hint` | `mobile_bots_none_check_again` |
| Bots tab, other host states | the state's existing text (`mobile_bots_unsupported`, `mobile_bots_offline_chats_note`, `mobile_bots_connect_to_load`) | none | `common_retry` for `mobile_bots_chats_failed` |

| Client | Translation |
| --- | --- |
| iOS | `ContentUnavailableView(title, systemImage:, description:)` with its `actions` closure. |
| Android | The shared `TeamsMessageState` (icon, title, body, action) for Blocked people and Bots too. |
| Desktop and Web | `.tw-empty`: h2 19px, body 13px muted, one button; `MemberHome` and the Friends main area use it. |

**U10 Motion.** No new animation. Reduce motion is respected everywhere.

| Client | Rule |
| --- | --- |
| iOS | `withAnimation` and `easeInOut` for section toggles and scroll-to are gated by `@Environment(\.accessibilityReduceMotion)`. |
| Android | `rememberReduceMotion()` reads `Settings.Global.ANIMATOR_DURATION_SCALE == 0f` once; when set, use `scrollToItem` instead of `animateScrollToItem` and skip the bar's `AnimatedVisibility` transitions. |
| Desktop and Web | The existing `prefers-reduced-motion` rules stay; the Processing spinner of U4 stops under them. |

**U11 Sections and headings.**
- One heading style per client: iOS native `Section` headers (Friends list headings stop being `.title3` rows); Android
  `titleSmall`; Desktop and Web 12px weight 600 in `--tw-muted`.
- The Teams list sections are Channels, Direct messages (`teams_group_direct`), Bot chats (`teams_group_bot_chats`) and, for
  the team's shared Bots, "Bots" (`remote_bots`, not "Assistants").
- A collapsible section: the whole header row is the button and meets U6; its chevron is decorative (Android
  `contentDescription = null`); the state is spoken as a value through `teams_section_expanded` and `teams_section_collapsed`;
  the action names are `teams_section_expand` and `teams_section_collapse`.
- Menus have a label: the conversation menu `teams_conversation_menu`, the team menu `teams_team_menu`; the other-teams dot
  of the switcher speaks `teams_other_unread`; the Info button is `teams_conversation_details`.

**U12 Copy.** Rules for any text this pass touches; the changed strings are in Appendix B.
- No engineering words in the UI: identity, digest, rebind, unbind, "pending change". The artifact version line is
  `apps_version %lld` ("Version 3"); the digest is not shown.
- Separators such as the middle dot are punctuation, identical in all five locales, and stay in code (`join(" · ")`); only
  sentences are keys.
- "Bot" tag on Bot messages is `teams_bot_tag`; Android clipboard labels are `teams_clip_invite`, `teams_clip_message` and
  `friends_clip_link`; the offline banner action is `teams_reconnect` (not `mcp_reconnect`).
- The invite expiry is shown as a formatted date on every client (Android shows the raw ISO text today).
- Not fixed here, because the backend writes it: the Bot reply body `结果需要授权，请联系助手维护者。` in
  `apps/firebase/src/shared-bots.mjs` still says 助手, and it and its sibling `消息已更新，本次结果未发布。请重新提问。` are
  Chinese for every locale. A backend change is outside this pass.

**U13 Desktop and Web copy (rule only; nothing is migrated in this pass).** The views of `packages/teams` say every text twice
inline (`zh ? ... : ...`, `t(en, zh)`), so zh-Hant users read Simplified Chinese and ja and ko users read English.
1. No new inline pair. Text goes through the shared catalog in all five locales.
2. Reuse the mobile key when the copy is the same (`teams_*`, `friends_*`); a string only Desktop and Web need takes the prefix
   `tw_` (Teams Web) and is added with the migration.
3. `zh: boolean` props become `locale: string`; the Electron hosts pass the real locale.
4. Until a component migrates, its inline pair must stay a pair of the catalog: `apps/web/test/friends-strings.test.ts` checks that
   every inline `t(en, zh)` of the eight Friends views is verbatim a pair of `en.json` and `zh-Hans.json`, and
   `packages/teams/friends.test.ts` checks the `friends_error_*` texts. That is why this pass adds keys beside
   `teams_group_bot_dm`, `friends_blocked_empty`, `friends_link_reset_confirm`, `friends_link_off_confirm` and
   `friends_share_bot_stop` instead of rewriting them: those texts are still inline in `FriendChatView.tsx`, `BlockedList.tsx`,
   `AddFriendView.tsx` and `FriendBotsPanel.tsx`.
5. The migration (phase P4c of the polish plan) generates `packages/teams/i18n.generated.ts`, adds `useTeamsText(locale)`, and then deletes the
   superseded keys of Appendix C.

Inline copy that still contradicts U1 until then: "assistant" / 助手 in `MemberHome.tsx`, `BotDirectory.tsx`, `TeamSidebar.tsx`
and `errors.ts`; "Bot DMs" in `TeamSidebar.tsx`, `FriendChatView.tsx` and `workspace.ts`; "owner" for a Bot's maintainer in
`BotDirectory.tsx`, `Composer.tsx` and `NewConversationDialog.tsx`; 私聊 in `NewConversationDialog.tsx`.

## 2. Appendix A: strings added

Added to all five `resources/translations/*.json` (real translations; `generate.py` regenerated `Localizable.xcstrings` and the five
`values*/strings.xml`, never hand-edited): **69 entries** = 55 plain keys + 7 placeholder pairs (the iOS key with
`%@` and its Android `_fmt` sibling with `%1$s`, listed once below). Reused without change: `common_cancel`, `common_retry`,
`chat_status_completed`, `chat_status_failed`, `chat_status_unknown`, `remote_bots`, `apps_version %lld`,
`teams_recall_confirm`, `teams_recall_confirm_detail`, `remote_forget_help`, `friends_share_bot_stop`.

**Own message and Bot tag (U3, U12)**

| Key | en |
| --- | --- |
| `teams_you` | You |
| `teams_bot_tag` | Bot |

**Confirmations: team management (U8)**

| Key | en |
| --- | --- |
| `teams_confirm_leave_title %@` + `_fmt` | Leave “%@”? |
| `teams_confirm_leave_body` | You will lose access to its conversations and shared Bots. To come back, you need a new invite. |
| `teams_confirm_leave_button` | Leave |
| `teams_confirm_delete_title %@` + `_fmt` | Delete “%@”? |
| `teams_confirm_delete_body` | Everyone loses access to its conversations and shared Bots. This can’t be undone. |
| `teams_confirm_delete_button` | Delete |
| `teams_confirm_remove_title %@` + `_fmt` | Remove %@ from the team? |
| `teams_confirm_remove_body` | They lose access to its conversations and shared Bots. They can rejoin with a new invite. |
| `teams_confirm_remove_button` | Remove |
| `teams_confirm_promote_title %@` + `_fmt` | Make %@ an admin? |
| `teams_confirm_promote_body` | Admins can invite people and remove members. |
| `teams_confirm_promote_button` | Make admin |
| `teams_confirm_demote_title %@` + `_fmt` | Make %@ a member? |
| `teams_confirm_demote_body` | They will no longer be able to invite people or remove members. |
| `teams_confirm_demote_button` | Make member |
| `teams_confirm_revoke_title` | Revoke this invite? |
| `teams_confirm_revoke_body` | The link stops working right away. People who already joined stay in the team. |
| `teams_confirm_revoke_button` | Revoke |

**Confirmations: stop sharing, friend link, computer (U8)**

| Key | en |
| --- | --- |
| `teams_bot_stop_title %@` + `_fmt` | Stop sharing “%@”? |
| `teams_bot_stop_detail` | New requests to this Bot stop. Existing conversations stay. You can share it again from LingXi Desktop. |
| `friends_link_reset_title` | Reset your friend link? |
| `friends_link_reset_body` | The old link stops working immediately. |
| `friends_link_off_title` | Turn off your friend link? |
| `friends_link_off_body` | No one can add you with it until you create a new one. |
| `remote_forget_title` | Forget this computer? |
| `remote_forget_confirm` | Forget |

**Run card and usage (U4, U5)**

| Key | en |
| --- | --- |
| `teams_run_queued` | Queued |
| `teams_run_expired` | Expired |
| `teams_run_interrupted` | Interrupted |
| `teams_run_authorization_required` | Authorization required |
| `teams_run_queued_detail` | Waiting for the Bot’s Desktop to pick this up. |
| `teams_run_processing_detail` | The Bot is working on it. |
| `teams_run_failed_detail` | The Bot couldn’t finish this request. Send it again. |
| `teams_run_expired_detail` | The Bot’s Desktop didn’t pick this up in time. Send it again. |
| `teams_run_interrupted_detail` | The Bot’s Desktop stopped before finishing. Send a new request. |
| `teams_run_authorization_required_detail` | This result can’t be shown yet. Ask the Bot’s maintainer to authorize it. |
| `teams_run_revoked_detail` | You no longer have access to this Bot. |
| `teams_run_refresh` | Refresh status |
| `teams_usage_tokens_total %@` + `_fmt` | %@ tokens |

**Offline and empty states (U9, U12)**

| Key | en |
| --- | --- |
| `teams_reconnect` | Reconnect |
| `teams_empty_title` | No teams yet |
| `teams_empty_thread_hint` | Say hello to start the conversation. |
| `friends_blocked_empty_title` | No blocked people |
| `friends_blocked_empty_body` | People you block can’t find you or send you requests. They will appear here. |
| `mobile_bots_none_title` | No Bots to use remotely |
| `mobile_bots_none_hint` | In LingXi on your computer, turn on “Remote control” for a Bot and it will show up here. |
| `mobile_bots_none_check_again` | Check again |

**Sections and assistive labels (U6, U11)**

| Key | en |
| --- | --- |
| `teams_group_bot_chats` | Bot chats |
| `teams_section_expand` | Expand |
| `teams_section_collapse` | Collapse |
| `teams_section_expanded` | Expanded |
| `teams_section_collapsed` | Collapsed |
| `teams_conversation_menu` | Conversation options |
| `teams_team_menu` | Team options |
| `teams_other_unread` | Other teams have unread messages |
| `teams_uses_decrease` | Decrease uses |
| `teams_uses_increase` | Increase uses |

**Android clipboard labels (U12)**

| Key | en |
| --- | --- |
| `teams_clip_invite` | LingXi team invitation |
| `teams_clip_message` | LingXi team message |
| `friends_clip_link` | LingXi friend link |

## 3. Appendix B: strings changed in place

Only where U1 or U12 rewrites an existing English string; all five locales change together (zh-Hant, ja and ko were English
for `teams_disclosure`, which is now translated). `teams_conversation_dm` changes zh-Hans only. No test reads the catalog value of
any of these keys (checked against the iOS, Android, Desktop, Web and `packages/teams` tests).

| Key | Before (en) | After (en) |
| --- | --- | --- |
| `teams_disclosure` | Team data and messages are readable by the platform. A Bot’s owner can read conversations with their Bot. | Team data and messages are readable by the platform. A Bot’s maintainer can read conversations with their Bot. |
| `teams_conversation_dm` | zh-Hans only: 私聊 | 私信 |
| `teams_message_confirming` | Waiting for server confirmation. Retries keep the same message identity. | Waiting for confirmation. Retrying won’t send it twice. |
| `teams_message_failed` | This pending change was rejected. Discard it and try again. | This couldn’t be sent. Discard it and try again. |
| `teams_bot_disabled` | Disabled · needs rebinding | Sharing stopped |
| `teams_composer_note_bot_dm` | Messages ask this Bot automatically. Its owner can read this conversation. | Messages ask this Bot automatically. Its maintainer can read this conversation. |
| `teams_composer_lock_owner_reading` | You can read this conversation because you own its Bot, but only participants can send messages. | You can read this conversation because you maintain its Bot, but only participants can send messages. |
| `teams_bots_note` | Bots run on their owner’s Desktop. The owner can read every conversation containing their Bot. Team members cannot approve its tools. | Bots run on their maintainer’s Desktop. The maintainer can read every conversation containing their Bot. Team members cannot approve its tools. |
| `teams_bot_use_listed` | Only owner and listed members may use | Only the maintainer and listed members may use |
| `teams_bot_not_shared` | Not shared. Rebind on Desktop to enable new requests. | New requests are off. The maintainer can share this Bot again from LingXi Desktop. |
| `teams_new_bot_owner_note` | A Bot’s owner can read every conversation that includes their Bot. | A Bot’s maintainer can read every conversation that includes their Bot. |
| `teams_new_no_bots` | No Bots are available to you yet. Ask a Bot’s owner to share one, or share your own from Team settings. | No Bots are available to you yet. Ask a Bot’s maintainer to share one, or share your own from LingXi Desktop. |
| `teams_usage_missing` | Missing usage · cost unknown | Usage unavailable |
| `friends_bots_note` | A Bot runs on its owner’s Desktop. The owner can read every conversation with it. | A Bot runs on its maintainer’s Desktop. The maintainer can read every conversation with it. |

## 4. Appendix C: superseded and dead keys (kept; delete after the clients and the migration land)

Superseded, still used by a client today:

| Old key | Replaced by |
| --- | --- |
| `teams_confirm`, `teams_confirm_change`, `teams_confirm_delete`, `teams_confirm_leave` | the per-action keys of U8 |
| `teams_bot_pause` | `friends_share_bot_stop` |
| `teams_bot_unbind_detail` (Friends dialog) | `teams_bot_stop_detail` |
| `teams_assistants` | `remote_bots` as the section title |
| `teams_group_bot_dm` | `teams_group_bot_chats` (not rewritten in place: pinned by `friends-strings.test.ts`) |
| `teams_task_expired`, `teams_task_interrupted`, `teams_task_authorization_required` | `teams_run_*` label plus `_detail` |
| borrowed `cron_status_queued`, `mcp_reconnect`, `chat_run_expand`, `chat_run_collapse` | `teams_run_queued`, `teams_reconnect`, `teams_section_expand`, `teams_section_collapse` |
| `teams_usage_complete` | no prefix for a complete usage line |
| `friends_blocked_empty`, `mobile_bots_none` | the `_title` / `_body` / `_hint` keys of U9 |
| `friends_link_reset_confirm`, `friends_link_off_confirm` | `friends_link_reset_title` / `_body`, `friends_link_off_title` / `_body` (still inline in `AddFriendView.tsx`) |

No client names these today (candidates for the cleanup after the migration): `teams_bot_unbind`, `teams_bot_unbind_title`,
`teams_run_preparing`, `teams_run_recovery`, `teams_run_recovered` ("Owner", for a Bot), `teams_run_missed`, `teams_run_host_*`
(6), `teams_question_*` (9), `teams_recover_*` (9), `teams_error_answer_rejected`, `friends_share_bot_with`.

## 5. Appendix D: contrast numbers

WCAG 2.x relative luminance. The tint is the accent at 6 percent alpha composited over the opaque stream background; text is the
client's own text colour. Required: 4.5:1 for text, 3:1 for graphics.

| Client and theme | Text on the tinted row | Ratio (untinted) |
| --- | --- | --- |
| Desktop light, accent `#0066d6`, row `#f0f6fd` | primary `#1d1d1f` | 15.44 (16.83) |
| | muted `#636366` | 5.49 (5.99) |
| Desktop dark, accent `#0a84ff`, row `#1d242d` | primary `#f5f5f7` | 14.35 (15.29) |
| | muted `#b0b0b5` | 7.23 (7.71) |
| Desktop, `--teams-accent` (88 percent accent, 12 percent black) light `#005abc` / dark `#0974e0` | primary | 15.36 / 14.51 |
| | muted | 5.46 / 7.31 |
| Web site theme light / dark | ink | 12.93 / 14.01 |
| | muted | 6.67 / 10.43 |
| `packages/teams` fallbacks light / dark | foreground | 15.56 / 13.21 |
| | muted | 5.96 / 6.46 |
| iOS over `systemBackground` (stream default), worst of the 7 system accents, light / dark | `label` | 19.32 / 19.74 |
| | `secondaryLabel` | 3.33 / 6.31 (untinted 3.44 / 6.36) |
| iOS over `systemGroupedBackground`, light / dark | `label` | 17.37 / 19.74 |
| | `secondaryLabel` | 3.19 / 6.31 |
| Android (Material 3 baseline, the 6 accent tones), light / dark | `onSurface` | 14.70 / 12.91 |
| | `onSurfaceVariant` | 8.04 / 9.82 |

The one value under 4.5:1 is the iOS `secondaryLabel` in light mode: it is 3.44:1 without the tint, so the tint costs 0.11 and the
platform's own secondary label is the cause. Body text never uses it; time and metadata on a tinted row keep `.secondary`.

Run-card glyph tones on the card surface (graphics, 3:1):

| Theme | ok (Completed) | danger (Failed) | muted (other states) |
| --- | --- | --- | --- |
| Desktop light, on `#ffffff` | `#207c37` 5.25 | `#c9342c` 5.24 | `#636366` 5.99 |
| Desktop dark, on `#2c2c2e` | `#32d74b` 7.27 | `#ff8079` 5.72 | `#b0b0b5` 6.45 |
| `packages/teams` light, on `#ffffff` | `--tw-online` `#1f7a3a` 5.38 | `--tw-danger` `#b3261e` 6.54 | `--tw-muted` `#5a5e66` 6.51 |
| `packages/teams` dark, on `#25262b` | `#4ac26b` 6.63 | `#ff8f86` 6.85 | `#a3a7b0` 6.26 |
| Android M3 baseline light, on `#fef7ff` | the existing online colour | `error` `#ba1a1a` 6.14 | `onSurfaceVariant` 8.88 |
| Android M3 baseline dark, on `#141218` | the existing online colour | `error` `#ffb4ab` 10.95 | `onSurfaceVariant` 10.91 |
| iOS light, on `#ffffff` | system green `#34c759` 2.22 | system red `#ff3b30` 3.55 | `.secondary` 3.44 |
| iOS dark, on `#1c1c1e` | system green `#30d158` 8.42 | system red `#ff453a` 4.99 | `.secondary` 6.36 |

## 6. Appendix E: tests, gates and open items

- **Tests that read the catalog.** `apps/web/test/friends-strings.test.ts` (inline pairs of the Friends views are catalog pairs),
  `packages/teams/friends.test.ts` (`friends_error_*` en and zh-Hans texts), `apps/electron/test/localization.test.ts` and
  `native-localization.test.ts`. With the strings of this pass the first two and `native-localization.test.ts` pass;
  `localization.test.ts` stops on `apps_library`, which is not in the catalog at HEAD either (it lives in
  `localAppWorkspaceCatalog.ts`, which the test does not merge), so that failure is not caused by this pass.
- **Tests that assert old English literals** (not the catalog) and will change with the client work: the usage-line tests
  (`apps/ios/native/Tests/TeamTaskUsageTests.swift`, `.../teams/TeamsBotTextTest.kt`, `apps/web/test/teams-bots-ui.test.ts`) assert
  "Missing usage", "Partial usage", "Complete usage" and six decimals (U5); `apps/electron/test/fixtures/teams-sidebar-electron.mjs`
  asserts the heading "Bot DMs" (U1); `apps/web/test/friends-ui.test.ts` asserts the inline "A Bot runs on its owner's Desktop."
  of `BotDirectory.tsx` (U1); `apps/web/test/teams-workspace-ui.test.ts` (line 476) asserts the inline "Waiting for server
  confirmation" of `TeamConversationView.tsx` (Appendix B, `teams_message_confirming`). All of them read inline text in
  `packages/teams` or a fixed English word list in the mobile code, none reads the catalog, so none fails today.
- **`scripts/checks/check_i18n_pairing.py`** already fails on the tree before this pass (94 unpaired against a baseline of 93, the
  plural key `teams_bots_count %lld`; zh-Hant 254 against 235; en 20 against 19). This pass adds no unpaired key (every `%@` key has
  its `_fmt` sibling) and raises the identical-to-zh-Hans counts to zh-Hant 260, en 23, ja 43, ko 13 (baselines 235, 19, 42, 13), so
  ja is the one line this pass newly trips (43 against 42, from `teams_bot_tag`). All 11 additions are intentional (a shared word or
  format, or the product term "Bot") and are ready to paste into `INTENTIONAL_IDENTICAL_TO_BASE`; with them the counts return to the
  pre-pass 254, 20, 42 and 12, and the older overage (zh-Hant, en, the one unpaired key) is left as it was:

```python
    # Teams UI polish: the same characters or the same product word in both Chinese variants, or in every locale.
    ("zh-Hant", "teams_you"): "你",
    ("zh-Hant", "teams_confirm_remove_button"): "移除",
    ("zh-Hant", "teams_bot_disabled"): "已停止共享",
    ("zh-Hant", "teams_usage_tokens_total %@"): "%@ tokens",
    ("zh-Hant", "teams_usage_tokens_total_fmt"): "%1$s tokens",
    ("en", "teams_usage_tokens_total %@"): "%@ tokens",
    ("en", "teams_usage_tokens_total_fmt"): "%1$s tokens",
    ("zh-Hant", "teams_bot_tag"): "Bot",
    ("en", "teams_bot_tag"): "Bot",
    ("ja", "teams_bot_tag"): "Bot",
    ("ko", "teams_bot_tag"): "Bot",
```

- **Translation debt found, not changed (out of scope for an add-only pass).** In `teams_*` and `friends_*`, 36 zh-Hant, 48 ja and 48 ko
  values are still the English text: the team-management keys `teams_new` to `teams_confirm_leave` (including the role names
  `teams_owner`, `teams_admin`, `teams_member` and the buttons `teams_promote`, `teams_demote`, `teams_remove`, `teams_leave`,
  `teams_delete`, `teams_revoke`) and the task keys from `teams_recent_tasks` to `teams_task_view`. Nine zh-Hant values are
  Simplified Chinese (`teams_bot_owner %@` and its `_fmt`, `teams_recent_tasks`, `teams_artifact_open`, `teams_task_expired`,
  `teams_task_interrupted`, `teams_task_authorization_required`, `teams_error_request_quota`, `teams_task_view`). The ones that
  carry a glossary term should be fixed first:

| Key | zh-Hant | ja | ko |
| --- | --- | --- | --- |
| `teams_owner` | 擁有者 | オーナー | 소유자 |
| `teams_admin` | 管理員 | 管理者 | 관리자 |
| `teams_member` | 成員 | メンバー | 멤버 |
| `teams_bot_owner %@` / `_fmt` | 維護者：%@ | メンテナー: %@ | 메인테이너: %@ |

  Also: `teams_preview_you` is 我 in zh-Hant while `teams_you` is 你 (Taiwan chat apps write 你); align the old key when it is
  next touched.
