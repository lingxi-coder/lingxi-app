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
    case 'PUBLICATION_CONFIRMATION_REQUIRED':
      return t('Review and confirm the publication before continuing.', '请检查并确认发布内容后继续。');
    case 'PUBLISHED_INPUT_REQUIRED':
    case 'INPUT_GRANT_REQUIRED':
    case 'INPUT_AUDIENCE_NARROWER':
      return t('The selected resources cannot be shared with this audience. Choose authorized resources or a narrower audience.', '选定资料无法向此受众共享，请选择有权共享的资料或缩小受众。');
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
