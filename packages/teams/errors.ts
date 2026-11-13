import { friendsErrorKey, offerErrorKey, type OfferAction } from './friends.js';

/** User-facing text for Teams error codes. Shared by the workspace views; codes stay language-neutral until shown. */

const membership: Record<string, [string, string]> = {
  INVITE_UNAVAILABLE: ['This invite has expired, was revoked or has no uses left.', '邀请已过期、撤销或用尽。'],
  MEMBERSHIP_REQUIRED: ['Your membership has ended. Refresh or join with a new invite.', '你已不在此团队，请刷新或通过新邀请加入。'],
  STALE_MEMBERSHIP: ['Membership changed. Refresh before trying again.', '成员身份已变化，请刷新后重试。'],
  ROLE_REQUIRED: ['Your role cannot perform this action.', '当前角色无权执行此操作。'],
  TEAM_LIMIT: ['You can join up to 20 teams.', '最多可加入 20 个团队。'],
  MEMBER_LIMIT: ['This team has reached 200 members.', '团队已达到 200 人上限。'],
};

/** Team membership, invitation and listing errors. */
export function teamsErrorText(code: string, zh: boolean): string {
  return membership[code]?.[zh ? 1 : 0] ?? (zh ? '无法连接团队服务，请检查网络后重试。' : 'Could not connect to Teams. Check your connection and try again.');
}

/** Messaging, Bot and pending-command errors. */
export function messageErrorText(code: string, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) {
    return t('Access changed. Return to members and refresh before continuing.', '访问权限已变化，请返回成员页并刷新。');
  }
  if (/STORAGE|disk/iu.test(code)) {
    return t('Local message storage is unavailable. Check free space and try again.', '本地消息存储不可用，请检查空间后重试。');
  }
  switch (code) {
    case 'BOT_USE_REQUIRED':
    case 'BOT_PARTICIPANT_REQUIRED':
    case 'INVALID_BOT_TARGET':
      return t('Bot access changed. Sync and check the selected Bots before sending again.', 'Bot 权限已变化，请同步并检查所选 Bot 后重试。');
    case 'TEAM_MODEL_UNSUPPORTED':
      return t('This model configuration cannot be used for Team Bots. Check the provider settings on Desktop or choose another model.', '当前模型配置不能用于团队 Bot，请检查 Desktop 的供应商配置或选择其他模型。');
    case 'TEAM_MODEL_UNAVAILABLE':
      return t('This Bot’s model or API key is unavailable. Check its model and provider settings on Desktop.', '此 Bot 的模型或 API 密钥不可用，请检查 Desktop 的模型与供应商设置。');
    case 'TEAM_RUNTIME_UNAVAILABLE':
      return t('The isolated Teams runtime is unavailable on this Desktop. Install a Desktop build that includes it.', '此 Desktop 缺少团队隔离运行时，请安装包含此运行时的 Desktop 版本。');
    case 'TEAM_HOST_MACOS_REQUIRED':
      return t('Sharing a Team Bot currently requires macOS Desktop.', '共享团队 Bot 当前需要 macOS Desktop。');
    case 'HOST_UNAVAILABLE':
    case 'TEAM_HOST_UNAVAILABLE':
      return t('The assistant’s host is unavailable. Ask its maintainer to reconnect Desktop.', '助手宿主不可用，请联系维护者重新连接 Desktop。');
    case 'INVALID_DATA':
      return t('Some of this team’s data is in an unsupported format and cannot be shown.', '此团队的部分数据格式不受支持，无法显示。');
    case 'REQUEST_QUOTA_EXCEEDED':
      return t('The daily request quota is exhausted. Ask the maintainer to review quotas or try after the UTC day resets.', '每日请求额度已用尽，请联系维护者检查额度或在 UTC 日期切换后重试。');
    case 'ARTIFACT_READ_REQUIRED':
      return t('You no longer have access to this document. Ask the maintainer to review its audience.', '你已无权访问此文档，请联系维护者检查受众。');
    case 'ARTIFACT_REVISION_CONFLICT':
      return t('This document was changed by someone else. Open it again to see the latest version.', '此文档已被其他人修改，请重新打开查看最新版本。');
    case 'ARTIFACT_UNCHANGED':
      return t('There are no changes to save.', '没有需要保存的修改。');
    case 'ARTIFACT_REVISION_LIMIT':
      return t('This document has reached its limit of 200 revisions and can no longer be edited.', '此文档已达到 200 个版本的上限，无法继续编辑。');
    case 'ARTIFACT_REVISION_REQUIRED':
      return t('That revision is not available.', '该版本不可用。');
    case 'LIST_REQUIRED':
      return t('This list is no longer available.', '此清单已不可用。');
    case 'LIST_REVISION_CONFLICT':
    case 'LIST_ITEM_REQUIRED':
      return t('The list was changed in the meantime. Refresh it and try again.', '清单在此期间已被修改，请刷新后重试。');
    case 'LIST_LIMIT':
      return t('The list is full. Remove some items before adding more.', '清单已满，请先移除一些条目再添加。');
    case 'SHARED_LIST_UNSUPPORTED':
      return t('This Bot’s model cannot keep a shared list. Choose another model for the Bot, or publish it without the list.', '此 Bot 使用的模型不支持共享清单。请为 Bot 更换模型，或不带清单发布。');
    case 'OPERATION_CONFLICT':
      return t('This was already decided differently. Reload to see the current state.', '此项已有不同的处理结果，请重新加载查看当前状态。');
    case 'OPERATION_REQUIRED':
    case 'OPERATION_READ_FORBIDDEN':
      return t('These actions are not available to you.', '你无法查看这些操作。');
    case 'MAINTAINER_REQUIRED':
      return t('Only the Bot’s maintainer can confirm what happened.', '只有 Bot 维护者可以确认实际结果。');
    case 'TASK_RUNNING':
      return t('The task is still running and may report the result itself. Try again after it stops.', '任务仍在运行，可能会自行报告结果。请在任务停止后重试。');
    case 'PUBLICATION_CONFIRMATION_REQUIRED':
      return t('Review and confirm the publication before continuing.', '请检查并确认发布内容后继续。');
    case 'PUBLISHED_INPUT_REQUIRED':
    case 'INPUT_GRANT_REQUIRED':
    case 'INPUT_REVISION_CONFLICT':
    case 'SHARED_RESOURCE_REQUIRED':
      return t('The shared files changed in the meantime. Choose the Bot again and review what it shares.', '共享的资料在此期间已更改。请重新选择 Bot 并检查共享内容。');
    case 'SHARED_RESOURCE_KIND':
      return t('A Skill can only be replaced by a SKILL.md file, and a file by another file.', 'Skill 只能用 SKILL.md 替换，文件只能用其他文件替换。');
    case 'SHARED_FILE_LIMIT':
      return t('Too much is shared at once. Stop sharing some files or choose smaller ones.', '共享的资料过多。请停止共享部分文件，或选择更小的文件。');
    case 'INPUT_AUDIENCE_NARROWER':
      return t('The selected resources cannot be shared with this audience. Choose authorized resources or a narrower audience.', '选定资料无法向此受众共享，请选择有权共享的资料或缩小受众。');
    case 'WAIT_KIND_MISMATCH':
      return t('This request needs a different kind of reply. Refresh and try again.', '此请求需要另一种回复方式，请刷新后重试。');
    case 'APPROVAL_REQUIRED':
    case 'APPROVAL_UNSUPPORTED':
      return t('This change needs an approval it does not have, so it was not made.', '此修改需要批准但尚未获批，因此未执行。');
    case 'WAIT_CLOSED':
    case 'WAIT_REQUIRED':
      return t('This question is no longer waiting for a reply.', '此问题已不再等待回复。');
    case 'WAIT_RECIPIENT_REQUIRED':
      return t('Only the person who asked the Bot can answer this question.', '只有向 Bot 提出请求的人才能回答此问题。');
    case 'TASK_REVOKED':
      return t('The Bot task lost its authorization, so it cannot continue.', '此 Bot 任务的授权已失效，无法继续。');
    case 'TASK_CANCEL_FORBIDDEN':
      return t('Only the person who asked, or the Bot’s maintainer, can cancel this task.', '只有发起请求的人或 Bot 维护者可以取消此任务。');
    case 'BOT_REVISION_CONFLICT':
      return t('Bot settings changed. Refresh before saving again.', 'Bot 设置已变化，请刷新后重试。');
    case 'MESSAGE_RECALLED':
    case 'MESSAGE_ID_CONFLICT':
    case 'MESSAGE_AUTHOR_REQUIRED':
    case 'PARTICIPANT_REQUIRED':
    case 'CONVERSATION_REJECTED':
    case 'INVALID_INPUT':
      return t('This pending change was rejected. Discard it and try again.', '此待发操作未被接受，请丢弃后重试。');
    default:
      return t('Could not sync messages. Check your connection and try again.', '消息同步未完成，请检查网络后重试。');
  }
}

/** The `en` and `zh-Hans` values of the canonical `friends_error_*` strings, by the key `friendsErrorKey` gives. */
const friends: Record<string, [string, string]> = {
  friends_error_not_found: ['No match found. Check the spelling, or ask them for a friend link.', '没有找到匹配的人。请检查拼写，或向对方索取好友链接。'],
  friends_error_rate_limited: ['Too many lookups or requests. Try again in a few minutes.', '查找或添加过于频繁，请几分钟后再试。'],
  friends_error_friend_limit: ['You or they have reached the friend limit (500).', '你或对方的好友数量已达上限（500）。'],
  friends_error_request_limit: ['You have 50 requests waiting. Cancel one first.', '待处理的请求已达 50 个，请先撤销一个。'],
  friends_error_handle_taken: ['That username is taken.', '该用户名已被占用。'],
  friends_error_request_unavailable: ['That request is no longer available.', '该请求已不存在。'],
  friends_error_stale_request: ['Something changed since you started. Look them up again.', '状态已发生变化，请重新查找对方。'],
  friends_error_session: ['Your session expired. Sign in again.', '登录已过期，请重新登录。'],
  friends_error_network: ["Can't reach the server. Check your connection and try again.", '无法连接服务器，请检查网络后重试。'],
  friends_error_invalid_input: ['Check what you entered and try again.', '请检查输入内容后重试。'],
  friends_error_generic: ['Something went wrong. Try again.', '出了点问题，请重试。'],
  friends_chat_unavailable: ['This chat is no longer available.', '此聊天已不可用。'],
};
/** The offer keys of D30, by the key `offerErrorKey` gives. */
const offers: Record<string, [string, string]> = {
  friends_error_friend_required: ['You can only invite people who are still your friends.', '只能邀请仍是你好友的人。'],
  friends_error_already_member: ['They are already in this team.', '对方已在此团队中。'],
  friends_error_offer_limit: ['They already have 20 invitations waiting. Try again later.', '对方已有 20 个待处理的邀请，请稍后再试。'],
  friends_error_member_limit: ['This team has reached 200 members.', '团队已达到 200 人上限。'],
  friends_error_team_limit: ['You can join up to 20 teams. Leave one to accept this invitation.', '最多可加入 20 个团队。请先退出一个团队再接受邀请。'],
  friends_error_offer_role: ['Only a team owner or admin can invite friends.', '只有团队所有者或管理员可以邀请好友。'],
  friends_error_offer_team_gone: ['You are no longer a member of this team.', '你已不在此团队中。'],
  friends_error_offer_unavailable: ['This invitation is no longer available.', '此邀请已失效。'],
};

/** Friends errors (D18): shown under the control that failed. Unknown codes read as the generic failure. */
export function friendsErrorText(code: string, zh: boolean): string {
  return friends[friendsErrorKey(code)]![zh ? 1 : 0];
}

/** Offer errors (D30): the three offer actions have their own table, which shares the common texts of the friends errors. */
export function offerErrorText(action: OfferAction, code: string, zh: boolean): string {
  const key = offerErrorKey(action, code);
  return (offers[key] ?? friends[key]!)[zh ? 1 : 0];
}
