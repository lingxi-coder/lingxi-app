import { friendsErrorKey, offerErrorKey, type OfferAction } from './friends.js';
import { teamsText, tk, type TeamsKey } from './i18n.js';

/** User-facing text for Teams error codes. Shared by the workspace views; codes stay language-neutral until shown. */

const membership: Record<string, TeamsKey> = {
  INVITE_UNAVAILABLE: tk('teams_invite_error'),
  MEMBERSHIP_REQUIRED: tk('tw_error_membership_ended'),
  STALE_MEMBERSHIP: tk('teams_stale_error'),
  ROLE_REQUIRED: tk('tw_error_role'),
  OWNER_REQUIRED: tk('tw_error_owner_only'),
  TEAM_LIMIT: tk('tw_error_team_limit'),
  MEMBER_LIMIT: tk('friends_error_member_limit'),
};

/** Team membership, invitation and listing errors. */
export function teamsErrorText(code: string, locale: string): string {
  return teamsText(locale)(Object.prototype.hasOwnProperty.call(membership, code) ? membership[code]! : 'teams_error');
}

/** Messaging, Bot and pending-command errors. */
export function messageErrorText(code: string, locale: string): string {
  const t = teamsText(locale);
  if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) {
    return t('tw_error_access_changed');
  }
  if (/STORAGE|disk/iu.test(code)) {
    return t('teams_messages_storage_error');
  }
  switch (code) {
    case 'BOT_USE_REQUIRED':
    case 'BOT_PARTICIPANT_REQUIRED':
    case 'INVALID_BOT_TARGET':
      return t('teams_error_bot_access_changed');
    case 'TEAM_MODEL_UNSUPPORTED':
      return t('tw_error_model_config');
    case 'TEAM_MODEL_UNAVAILABLE':
      return t('tw_error_model_key');
    case 'TEAM_RUNTIME_UNAVAILABLE':
      return t('tw_error_runtime');
    case 'TEAM_HOST_MACOS_REQUIRED':
      return t('tw_error_macos');
    case 'HOST_UNAVAILABLE':
    case 'TEAM_HOST_UNAVAILABLE':
      return t('tw_error_host');
    case 'INVALID_DATA':
      return t('teams_error_invalid_data');
    case 'REQUEST_QUOTA_EXCEEDED':
      return t('tw_error_quota');
    case 'ATTACHMENT_TYPE':
    case 'UPLOAD_TYPE_MISMATCH':
      return t('teams_file_error_type');
    case 'ATTACHMENT_EMPTY':
      return t('teams_file_error_empty');
    case 'ATTACHMENT_TOO_LARGE':
      return t('teams_file_error_too_large');
    case 'ATTACHMENT_COUNT':
      return t('teams_file_error_count');
    case 'ATTACHMENT_TOTAL':
    case 'ATTACHMENT_LIMIT':
      return t('teams_file_error_total');
    case 'UPLOAD_QUOTA_EXCEEDED':
      return t('teams_file_error_quota');
    case 'UPLOAD_EXPIRED':
      return t('teams_file_error_expired');
    case 'UPLOAD_DIGEST_MISMATCH':
    case 'UPLOAD_INCOMPLETE':
    case 'UPLOAD_PART_INVALID':
    case 'UPLOAD_FAILED':
      return t('teams_file_error_incomplete');
    case 'UPLOAD_REQUIRED':
    case 'UPLOAD_SEALED':
    case 'UPLOAD_ID_CONFLICT':
    case 'ATTACHMENT_REQUIRED':
      return t('teams_file_error_unsendable');
    case 'ATTACHMENT_REVOKED':
      return t('teams_file_error_revoked');
    case 'ATTACHMENT_DAMAGED':
      return t('teams_file_error_damaged');
    case 'ARTIFACT_READ_REQUIRED':
      return t('tw_error_doc_access');
    case 'ARTIFACT_REVISION_CONFLICT':
      return t('teams_error_doc_conflict');
    case 'ARTIFACT_UNCHANGED':
      return t('teams_error_doc_unchanged');
    case 'ARTIFACT_REVISION_LIMIT':
      return t('teams_error_doc_limit');
    case 'ARTIFACT_REVISION_REQUIRED':
      return t('teams_error_doc_revision');
    case 'LIST_REQUIRED':
      return t('teams_error_list_unavailable');
    case 'LIST_REVISION_CONFLICT':
    case 'LIST_ITEM_REQUIRED':
      return t('tw_error_list_changed');
    case 'LIST_LIMIT':
      return t('teams_error_list_full');
    case 'SHARED_LIST_UNSUPPORTED':
      return t('tw_error_list_model');
    case 'OPERATION_CONFLICT':
      return t('teams_error_ops_conflict');
    case 'OPERATION_REQUIRED':
    case 'OPERATION_READ_FORBIDDEN':
      return t('teams_error_ops_unavailable');
    case 'MAINTAINER_REQUIRED':
      return t('teams_error_ops_maintainer');
    case 'TASK_RUNNING':
      return t('teams_error_ops_running');
    case 'PUBLICATION_CONFIRMATION_REQUIRED':
      return t('tw_error_review_publication');
    case 'PUBLISHED_INPUT_REQUIRED':
    case 'INPUT_GRANT_REQUIRED':
    case 'INPUT_REVISION_CONFLICT':
    case 'SHARED_RESOURCE_REQUIRED':
      return t('tw_error_files_changed');
    case 'SHARED_RESOURCE_KIND':
      return t('tw_error_replace_kind');
    case 'SHARED_FILE_LIMIT':
      return t('tw_error_too_much');
    case 'INPUT_AUDIENCE_NARROWER':
      return t('tw_error_audience');
    case 'WAIT_KIND_MISMATCH':
      return t('tw_error_reply_kind');
    case 'APPROVAL_REQUIRED':
    case 'APPROVAL_UNSUPPORTED':
      return t('tw_error_no_approval');
    case 'WAIT_CLOSED':
    case 'WAIT_REQUIRED':
      return t('tw_error_question_closed');
    case 'WAIT_RECIPIENT_REQUIRED':
      return t('tw_error_question_recipient');
    case 'TASK_REVOKED':
      return t('tw_error_task_authorization');
    case 'TASK_CANCEL_FORBIDDEN':
      return t('tw_error_cancel_who');
    case 'BOT_MAINTAINER_CHANGED':
      return t('tw_error_handed_over');
    case 'TRANSFER_PENDING':
      return t('tw_error_already_offered');
    case 'TRANSFER_ENDED':
    case 'TRANSFER_REQUIRED':
    case 'TRANSFER_COMPLETED':
    case 'TRANSFER_PARTY_REQUIRED':
    case 'TRANSFER_RECIPIENT_REQUIRED':
      return t('tw_error_handover_closed');
    case 'CURRENT_RECIPIENT_REQUIRED':
      return t('tw_error_not_member');
    case 'TRANSFER_PACKAGE_MISMATCH':
      return t('tw_error_handover_mismatch');
    case 'TRANSFER_PACKAGE_REQUIRED':
    case 'TRANSFER_SEALED':
    case 'TRANSFER_ID_CONFLICT':
      return t('tw_error_offer_failed');
    case 'SHARED_BOT_REQUIRED':
      return t('tw_error_other_computer');
    case 'BOT_PAUSED':
      return t('tw_error_paused');
    case 'BOT_REVISION_CONFLICT':
      return t('tw_error_settings_changed');
    case 'MESSAGE_RECALLED':
    case 'MESSAGE_ID_CONFLICT':
    case 'MESSAGE_AUTHOR_REQUIRED':
    case 'PARTICIPANT_REQUIRED':
    case 'CONVERSATION_REJECTED':
    case 'INVALID_INPUT':
      return t('teams_message_failed');
    default:
      return t('teams_error_sync_failed');
  }
}

/** Friends errors (D18): shown under the control that failed. Unknown codes read as the generic failure. */
export function friendsErrorText(code: string, locale: string): string {
  return teamsText(locale)(friendsErrorKey(code));
}

/** Offer errors (D30): the three offer actions have their own table in `friends.ts`, which shares the common texts of the friends errors. */
export function offerErrorText(action: OfferAction, code: string, locale: string): string {
  return teamsText(locale)(offerErrorKey(action, code));
}
