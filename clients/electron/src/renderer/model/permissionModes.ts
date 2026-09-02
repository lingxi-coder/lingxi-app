export interface PermissionModeOption {
  id: 'default' | 'acceptEdits' | 'plan' | 'auto' | 'dontAsk' | 'bypassPermissions';
  label: string;
  shortLabel: string;
  description: string;
  icon: 'hand' | 'pencil' | 'file' | 'shieldCheck' | 'lock' | 'shieldAlert';
  danger?: boolean;
}

export const PERMISSION_MODE_OPTIONS: readonly PermissionModeOption[] = [
  { id: 'default', label: 'Ask for approval', shortLabel: 'Ask', description: 'Ask before edits, commands, and external actions', icon: 'hand' },
  { id: 'acceptEdits', label: 'Accept edits', shortLabel: 'Accept edits', description: 'Apply file edits automatically; ask for other actions', icon: 'pencil' },
  { id: 'plan', label: 'Plan mode', shortLabel: 'Plan', description: 'Explore and create a plan without making changes', icon: 'file' },
  { id: 'auto', label: 'Approve for me', shortLabel: 'Auto', description: 'Automatically approve safe actions and ask on risk', icon: 'shieldCheck' },
  { id: 'dontAsk', label: "Don't ask", shortLabel: "Don't ask", description: 'Deny actions that would otherwise require approval', icon: 'lock' },
  { id: 'bypassPermissions', label: 'Full access', shortLabel: 'Full access', description: 'Unrestricted access to tools and files on this computer', icon: 'shieldAlert', danger: true },
];
