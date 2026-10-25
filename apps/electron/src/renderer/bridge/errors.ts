import { tr } from '../i18n/locale';

export interface ActionableError {
  readonly title: string;
  readonly detail: string;
  readonly technicalDetail?: string;
}

export function classifyDesktopError(raw: string): ActionableError {
  const message = raw.trim() || tr('desktop_misc_unknown_error');
  if (/ENOSPC|No space left on device|os error 28|disk full|disk space is insufficient/i.test(message)) {
    return {
      title: tr('desktop_misc_disk_full'),
      detail: tr('desktop_misc_disk_error_hint'),
    };
  }
  if (/Data Protection Keychain.*unavailable|keychain access group|app signature/i.test(message)) {
    return { title: tr('desktop_misc_secure_unavailable'), detail: tr('desktop_misc_keychain_reconnect') };
  }
  if (/could not be decrypted|secure credential storage|keychain/i.test(message)) {
    return {
      title: tr('desktop_misc_credential_unavailable'),
      detail: tr('desktop_misc_keychain_replace'),
    };
  }
  if (/credential required|no accepted credential source|configure a trusted credential source/i.test(message)) {
    return { title: tr('desktop_misc_credential_missing'), detail: tr('desktop_misc_reconnect_provider') };
  }
  // Codex signs in with OAuth; it has no stored API key to replace.
  if (/Codex authentication.*Sign in again/i.test(message)) {
    return { title: tr('desktop_misc_codex_signin'), detail: message };
  }
  if (/401|403|unauthori[sz]ed|api[-_ ]?key|credential|authentication/i.test(message)) {
    return { title: tr('desktop_misc_credential_rejected'), detail: tr('desktop_misc_replace_key') };
  }
  if (/protocol|handshake|version mismatch|incompatible/i.test(message)) {
    return { title: tr('desktop_misc_engine_version'), detail: tr('desktop_misc_install_matching') };
  }
  if (/binary|bridge-server.*(missing|not found)|ENOENT/i.test(message)) {
    return { title: tr('desktop_misc_engine_unavailable'), detail: tr('desktop_misc_reinstall') };
  }
  if (/unexpected result:.*(?:invalid type|valid JSON value)|invalid type: newtype struct/i.test(message)) {
    return {
      title: tr('desktop_misc_unreadable_response'),
      detail: tr('desktop_misc_unexpected_response'),
      technicalDetail: message,
    };
  }
  if (/workspace|directory|cwd|folder/i.test(message)) {
    return { title: tr('desktop_misc_workspace_unavailable'), detail: tr('desktop_misc_choose_workspace') };
  }
  if (/socket|transport|connect|disconnected|exited|timed out/i.test(message)) {
    return { title: tr('desktop_misc_engine_interrupted'), detail: tr('desktop_misc_restart_engine') };
  }
  return { title: tr('desktop_misc_action_failed'), detail: message };
}
