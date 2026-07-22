export interface ActionableError {
  readonly title: string;
  readonly detail: string;
}

export function classifyDesktopError(raw: string): ActionableError {
  const message = raw.trim() || 'Unknown desktop error';
  if (/could not be decrypted|secure credential storage|keychain/i.test(message)) {
    return {
      title: 'Provider credential unavailable',
      detail: 'Allow LingXi Code in Keychain, or replace the stored API key in Settings, then retry.',
    };
  }
  if (/credential required|no accepted credential source|configure a trusted credential source/i.test(message)) {
    return { title: 'Provider credential missing', detail: 'Reconnect this provider in Settings, then retry.' };
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
