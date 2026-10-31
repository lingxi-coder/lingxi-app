export const CH_ACCOUNT = 'lingxi:account';
export const CH_ACCOUNT_CHANGED = 'lingxi:account:changed';

export interface AccountUser {
  uid: string;
  email: string | null;
  displayName: string | null;
  emailVerified: boolean;
}
export interface AccountState {
  user: AccountUser | null;
  busy: boolean;
  persistence: 'secure' | 'session';
  error: string | null;
  /** A previously saved/signed-in session must be authenticated again. */
  sessionExpired?: boolean;
}
export type AccountCommand =
  | { type: 'status' | 'signOut' | 'cancel' | 'verifyEmail' | 'reload' }
  | { type: 'signIn' | 'register'; email: string; password: string }
  | { type: 'resetPassword'; email: string }
  | { type: 'oauth'; provider: 'google.com' | 'apple.com' };
export interface AccountApi {
  request(command: AccountCommand): Promise<AccountState>;
  onChanged(callback: (state: AccountState) => void): () => void;
}

export function parseAccountCommand(value: unknown): AccountCommand {
  if (!value || typeof value !== 'object') throw new Error('Invalid account command');
  const input = value as Record<string, unknown>;
  if (['status', 'signOut', 'cancel', 'verifyEmail', 'reload'].includes(String(input.type))) {
    return { type: input.type } as AccountCommand;
  }
  if (input.type === 'oauth' && ['google.com', 'apple.com'].includes(String(input.provider))) {
    return { type: 'oauth', provider: input.provider as 'google.com' | 'apple.com' };
  }
  if (['signIn', 'register', 'resetPassword'].includes(String(input.type))) {
    if (typeof input.email !== 'string' || input.email.trim().length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(input.email.trim())) {
      throw new Error('请输入有效的邮箱地址。');
    }
    const email = input.email.trim();
    if (input.type === 'resetPassword') return { type: input.type, email };
    if (typeof input.password !== 'string' || !input.password || input.password.length > 4096) throw new Error('请输入密码。');
    return { type: input.type as 'signIn' | 'register', email, password: input.password };
  }
  throw new Error('Invalid account command');
}
