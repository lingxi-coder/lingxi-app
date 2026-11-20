export const CH_ACCOUNT = 'lingxi:account';
export const CH_ACCOUNT_CHANGED = 'lingxi:account:changed';

export interface AccountUser {
  uid: string;
  email: string | null;
  displayName: string | null;
  emailVerified: boolean;
  /**
   * How this account signs in (`password`, `google.com`, `apple.com`), from the account's linked providers; absent until
   * the host has looked them up. Re-authentication offers only these.
   */
  signInMethods?: AccountSignInMethod[];
}
export type AccountSignInMethod = 'password' | 'google.com' | 'apple.com';
export const ACCOUNT_SIGN_IN_METHODS: readonly AccountSignInMethod[] = ['password', 'google.com', 'apple.com'];
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
  /** `expectUid` re-authenticates the signed-in account: another account is refused and the session stays as it was. */
  | { type: 'signIn'; email: string; password: string; expectUid?: string }
  | { type: 'register'; email: string; password: string }
  | { type: 'resetPassword'; email: string }
  | { type: 'oauth'; provider: 'google.com' | 'apple.com'; expectUid?: string };
export interface AccountApi {
  request(command: AccountCommand): Promise<AccountState>;
  onChanged(callback: (state: AccountState) => void): () => void;
}

export function parseAccountCommand(value: unknown): AccountCommand {
  if (!value || typeof value !== 'object') throw new Error('Invalid account command');
  const input = value as Record<string, unknown>;
  if (input.expectUid !== undefined && (!['signIn', 'oauth'].includes(String(input.type)) || typeof input.expectUid !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(input.expectUid))) {
    throw new Error('Invalid account command');
  }
  const expect = typeof input.expectUid === 'string' ? { expectUid: input.expectUid } : {};
  if (['status', 'signOut', 'cancel', 'verifyEmail', 'reload'].includes(String(input.type))) {
    return { type: input.type } as AccountCommand;
  }
  if (input.type === 'oauth' && ['google.com', 'apple.com'].includes(String(input.provider))) {
    return { type: 'oauth', provider: input.provider as 'google.com' | 'apple.com', ...expect };
  }
  if (['signIn', 'register', 'resetPassword'].includes(String(input.type))) {
    if (typeof input.email !== 'string' || input.email.trim().length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(input.email.trim())) {
      throw new Error('请输入有效的邮箱地址。');
    }
    const email = input.email.trim();
    if (input.type === 'resetPassword') return { type: input.type, email };
    if (typeof input.password !== 'string' || !input.password || input.password.length > 4096) throw new Error('请输入密码。');
    return input.type === 'signIn' ? { type: 'signIn', email, password: input.password, ...expect } : { type: 'register', email, password: input.password };
  }
  throw new Error('Invalid account command');
}
