export interface ActionableError {
  readonly title: string;
  readonly detail: string;
}

export function classifyDesktopError(raw: string): ActionableError {
  const message = raw.trim() || 'Unknown desktop error';
  if (/ENOSPC|No space left on device|os error 28|disk full|disk space is insufficient/i.test(message)) {
    return {
      title: '磁盘空间不足，任务已暂停',
      detail: '释放磁盘空间后，点击“重试并继续”恢复当前会话。已有历史记录会保留。',
    };
  }
  if (/Data Protection Keychain.*unavailable|keychain access group|app signature/i.test(message)) {
    return { title: 'Secure persistence unavailable', detail: 'Allow LingXi Code to access Keychain, then reconnect the provider.' };
  }
  if (/could not be decrypted|secure credential storage|keychain/i.test(message)) {
    return {
      title: 'Provider credential unavailable',
      detail: 'Allow LingXi Code in Keychain, or replace the stored API key in Settings, then retry.',
    };
  }
  if (/credential required|no accepted credential source|configure a trusted credential source/i.test(message)) {
    return { title: 'Provider credential missing', detail: 'Reconnect this provider in Settings, then retry.' };
  }
  // Codex signs in with OAuth; it has no stored API key to replace.
  if (/Codex authentication.*Sign in again/i.test(message)) {
    return { title: 'Codex sign-in required', detail: message };
  }
  if (/401|403|unauthori[sz]ed|api[-_ ]?key|credential|authentication/i.test(message)) {
    return { title: 'Provider credential rejected', detail: 'Replace the stored API key in Settings, then retry.' };
  }
  if (/protocol|handshake|version mismatch|incompatible/i.test(message)) {
    return { title: 'Engine version mismatch', detail: 'Install a matching LingXi Code Beta build.' };
  }
  if (/binary|bridge-server.*(missing|not found)|ENOENT/i.test(message)) {
    return { title: 'Bundled engine unavailable', detail: 'Reinstall the verified Beta artifact.' };
  }
  if (/workspace|directory|cwd|folder/i.test(message)) {
    return { title: 'Workspace unavailable', detail: 'Choose a readable workspace folder and review trust again.' };
  }
  if (/socket|transport|connect|disconnected|exited|timed out/i.test(message)) {
    return { title: 'Engine connection interrupted', detail: 'Restart the engine from Settings and retry.' };
  }
  return { title: 'LingXi could not complete the action', detail: message };
}
