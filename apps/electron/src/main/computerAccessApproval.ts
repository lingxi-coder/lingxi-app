import type { ComputerAccessRequestDto, ComputerAccessResponseDto } from '@lingxi/bridge-client';

/** Recheck the stored pending request at the trusted host boundary. */
export function assertComputerAccessSubset(request: ComputerAccessRequestDto, response: ComputerAccessResponseDto): void {
  const requested = new Set(request.apps.map(app => app.label));
  const hasGrants = response.granted_apps.length > 0 || response.clipboard_read || response.clipboard_write || response.system_key_combos;
  const kind = request.interaction?.kind;
  const action = response.interaction_action;
  const mode = response.preferred_mode;
  if (mode !== undefined && (mode !== 'background' && mode !== 'full_control'
      || request.tcc_state || kind !== undefined && (kind !== 'teach_access' || action !== 'allow'))) {
    throw new Error('Computer interaction preference exceeds the pending request');
  }
  const permittedAction = kind === 'teach_step' ? action === 'continue' || action === 'exit'
    : kind === 'takeover' || kind === 'teach_access' ? action === 'allow' : false;
  if (action !== undefined && (request.tcc_state || !permittedAction)
      || kind !== undefined && hasGrants && (kind !== 'teach_access' || action !== 'allow')) {
    throw new Error('Computer interaction approval exceeds the pending request');
  }
  if ((request.tcc_state && (response.granted_apps.length > 0 || response.clipboard_read || response.clipboard_write || response.system_key_combos))
      || response.granted_apps.some(app => !requested.has(app))
      || new Set(response.granted_apps).size !== response.granted_apps.length
      || response.clipboard_read && !request.clipboard_read
      || response.clipboard_write && !request.clipboard_write
      || response.system_key_combos && !request.system_key_combos) {
    throw new Error('Computer approval exceeds the pending request');
  }
}
