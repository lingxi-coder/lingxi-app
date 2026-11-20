/** The words for an inbox error code, for any view that lists the inbox. */
export function inboxErrorText(code: string, cn: boolean) {
  if (['INBOX_OUTBOX_FULL', 'QuotaExceededError', 'INBOX_STORAGE_FAILED'].includes(code))
    return cn
      ? '同步空间已满或不可用。请联网同步后重试。'
      : 'Sync storage is full or unavailable. Reconnect and retry.';
  if (
    ['SOURCE_ACCESS_REQUIRED', 'STALE_MEMBERSHIP', 'MEMBERSHIP_REQUIRED', 'INBOX_ITEM_MISSING', 'permission-denied'].includes(code)
  )
    return cn ? '当前账号无法再访问此会话。' : 'This account no longer has access to this conversation.';
  if (['INVALID_SESSION', 'SIGN_IN_REQUIRED', 'unauthenticated'].includes(code))
    return cn ? '请重新登录后重试。' : 'Sign in again to continue.';
  if (code === 'PREFERENCE_REVISION_CONFLICT')
    return cn
      ? '另一台设备已更改会话设置。请检查后重试。'
      : 'Another device changed these settings. Review the latest settings and retry.';
  if (code === 'INBOX_BATCH_EXPIRED' || code === 'INBOX_JOB_MISSING')
    return cn ? '批量已读请求已失效，请重试。' : 'The bulk read request expired. Please retry.';
  if (code === 'SOURCE_UNAVAILABLE')
    return cn ? '来源设备尚未共享此会话。' : 'The source device has not shared this conversation.';
  return cn ? '暂时无法同步。请检查网络后重试。' : 'Sync is unavailable. Check your connection and retry.';
}
