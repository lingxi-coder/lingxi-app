/** User-facing text for Teams error codes. Shared by the panels and the workspace views; codes stay language-neutral until shown. */

const membership: Record<string, [string, string]> = {
  INVITE_UNAVAILABLE: ['This invite has expired, was revoked or has no uses left.', '邀请已过期、撤销或用尽。'],
  MEMBERSHIP_REQUIRED: ['Your membership has ended. Refresh or join with a new invite.', '你已不在此团队，请刷新或通过新邀请加入。'],
  STALE_MEMBERSHIP: ['Membership changed. Refresh before trying again.', '成员身份已变化，请刷新后重试。'],
  ROLE_REQUIRED: ['Your role cannot perform this action.', '当前角色无权执行此操作。'],
  TEAM_LIMIT: ['You can join up to 20 teams.', '最多可加入 20 个团队。'],
  MEMBER_LIMIT: ['This team has reached 200 members.', '团队已达到 200 人上限。'],
};

/** Team membership, invitation and listing errors (the former TeamsPanel `translateError`). */
export function teamsErrorText(code: string, zh: boolean): string {
  return membership[code]?.[zh ? 1 : 0] ?? (zh ? '无法连接团队服务，请检查网络后重试。' : 'Could not connect to Teams. Check your connection and try again.');
}

/** Messaging, Bot and pending-command errors (the former MessagesPanel `errorText`). */
export function messageErrorText(code: string, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) {
    return t('Access changed. Return to members and refresh before continuing.', '访问权限已变化，请返回成员页并刷新。');
  }
  if (/STORAGE|Quota|disk/iu.test(code)) {
    return t('Local message storage is unavailable. Check free space and try again.', '本地消息存储不可用，请检查空间后重试。');
  }
  switch (code) {
    case 'BOT_USE_REQUIRED':
    case 'BOT_PARTICIPANT_REQUIRED':
    case 'INVALID_BOT_TARGET':
      return t('Bot access changed. Sync and check the selected Bots before sending again.', 'Bot 权限已变化，请同步并检查所选 Bot 后重试。');
    case 'QUESTION_RECIPIENT_REQUIRED':
    case 'ANSWER_CONFLICT':
    case 'INVALID_ANSWER':
      return t('This answer cannot be accepted. Check the question and original requester.', '此答案无法接收，请检查问题与原始请求人身份。');
    case 'TEAM_MODEL_UNSUPPORTED':
      return t('This model configuration cannot be used for Team Bots. Check the provider settings on Desktop or choose another model.', '当前模型配置不能用于团队 Bot，请检查 Desktop 的供应商配置或选择其他模型。');
    case 'TEAM_MODEL_UNAVAILABLE':
      return t('This Bot’s model or API key is unavailable. Check its model and provider settings on Desktop.', '此 Bot 的模型或 API 密钥不可用，请检查 Desktop 的模型与供应商设置。');
    case 'TEAM_RUNTIME_UNAVAILABLE':
      return t('The isolated Teams runtime is unavailable on this Desktop. Install a Desktop build that includes it.', '此 Desktop 缺少团队隔离运行时，请安装包含此运行时的 Desktop 版本。');
    case 'TEAM_HOST_MACOS_REQUIRED':
      return t('Sharing a Team Bot currently requires macOS Desktop.', '共享团队 Bot 当前需要 macOS Desktop。');
    case 'TEAM_HOST_UNAVAILABLE':
      return t('The Desktop Bot host is unavailable. Check the Desktop service and try again.', 'Desktop Bot 主机不可用，请检查 Desktop 服务后重试。');
    case 'RECOVERY_NOT_REQUIRED':
      return t('The run is not eligible for a new settlement. Refresh and review its current status.', '此运行当前无法创建新的处理结果，请刷新并检查状态。');
    case 'BOT_RESULT_CONFLICT':
      return t('A different settlement is already saved for this request. Retry the saved result unchanged.', '此请求已保存不同的处理结果，请重试原始结果。');
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
