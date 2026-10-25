import { currentLocale, translate } from '../i18n/locale';

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

/** Build display copy at render time so an interface-language change is reflected immediately. */
export function permissionModeOptions(locale = currentLocale()): readonly PermissionModeOption[] {
  const keys: Record<PermissionModeOption['id'], readonly [string, string, string]> = {
    default: ['desktop_chat_permission_ask', 'desktop_chat_permission_ask_short', 'desktop_chat_permission_ask_description'],
    acceptEdits: ['desktop_chat_permission_accept', 'desktop_chat_permission_accept', 'desktop_chat_permission_accept_description'],
    plan: ['desktop_chat_permission_plan', 'desktop_chat_permission_plan_short', 'desktop_chat_permission_plan_description'],
    auto: ['desktop_chat_permission_auto', 'composer_effort_auto', 'desktop_chat_permission_auto_description'],
    dontAsk: ['desktop_chat_permission_dont_ask', 'desktop_chat_permission_dont_ask', 'desktop_chat_permission_dont_ask_description'],
    bypassPermissions: ['desktop_chat_permission_full', 'desktop_chat_permission_full', 'desktop_chat_permission_full_description'],
  };
  return PERMISSION_MODE_OPTIONS.map((option) => {
    const [label, shortLabel, description] = keys[option.id];
    return { ...option, label: translate(label, locale), shortLabel: translate(shortLabel, locale), description: translate(description, locale) };
  });
}
