import { useEffect, useRef, useState } from 'react';
import type { AccountCommand, AccountState } from '../../../../shared/account';
import { Card } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle } from './ghostButton';

const googleIcon = new URL('../../../assets/google-g.png', import.meta.url).href;

export function LingxiAccount() {
  const t = useT();
  const [state, setState] = useState<AccountState | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [register, setRegister] = useState(false);
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState('');
  const inFlight = useRef(false);
  useEffect(() => {
    const api = window.lingxi?.account;
    if (!api) return;
    let active = true;
    let revision = 0;
    const off = api.onChanged(value => { revision++; if (active) setState(value); });
    void api.request({ type: 'status' }).then(value => {
      if (active && revision === 0) setState(value);
    }).catch(() => { if (active) setMessage('无法读取账户状态，请重新打开设置。'); });
    return () => { active = false; off(); };
  }, []);
  const busy = pending || Boolean(state?.busy);
  const run = async (command: AccountCommand) => {
    const api = window.lingxi?.account;
    if (!api || (inFlight.current && command.type !== 'cancel')) return;
    if (command.type === 'cancel') { await api.request(command).catch(() => undefined); return; }
    inFlight.current = true; setPending(true); setMessage('');
    try {
      const next = await api.request(command);
      setState(next);
      if (next.user || command.type === 'signOut') setPassword('');
      if (!next.error) {
        if (['signIn', 'register', 'oauth', 'signOut'].includes(command.type)) setPassword('');
        if (command.type === 'resetPassword') setMessage('如果此邮箱已注册，将收到密码重置邮件。');
        if (command.type === 'verifyEmail') setMessage('验证邮件已发送，请检查收件箱。');
      }
    } catch (error) { setMessage(error instanceof Error ? error.message : '操作未完成，请重试。'); }
    finally { inFlight.current = false; setPending(false); }
  };
  const button = ghostButtonStyle(t, busy);
  const inputStyle = { background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 8, padding: '10px 12px', width: '100%', boxSizing: 'border-box' as const, font: 'inherit' };
  return <Card title="LingXi 账户">
    <div style={{ padding: 18, display: 'grid', gap: 12, maxWidth: 460 }}>
      {!state ? <p role="status">正在读取账户…</p> : state.user ? <>
        <strong>{state.user.displayName || state.user.email || '已登录'}</strong>
        <span style={{ color: t.text3 }}>{state.user.email}</span>
        <span style={{ color: t.text3, fontSize: 12 }}>{state.user.emailVerified ? '邮箱已验证' : '邮箱尚未验证'}</span>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
          {!state.user.emailVerified && state.user.email && <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'verifyEmail' })}>发送验证邮件</button>}
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'reload' })}>刷新账户</button>
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'signOut' })}>退出登录</button>
        </div>
      </> : <>
        <form onSubmit={event => { event.preventDefault(); void run({ type: register ? 'register' : 'signIn', email, password }); }} style={{ display: 'grid', gap: 12 }}>
          <label>邮箱<input type="email" autoComplete="username" required value={email} onChange={e => setEmail(e.target.value)} disabled={busy} style={inputStyle} /></label>
          <label>密码<input type="password" autoComplete={register ? 'new-password' : 'current-password'} minLength={register ? 6 : undefined} required value={password} onChange={e => setPassword(e.target.value)} disabled={busy} style={inputStyle} /></label>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            <button type="submit" disabled={busy} style={button}>{register ? '创建账户' : '登录'}</button>
            <button type="button" disabled={busy} style={button} onClick={() => { setRegister(!register); setPassword(''); setMessage(''); }}>{register ? '已有账户，登录' : '注册新账户'}</button>
            <button type="button" disabled={busy || !email.trim()} style={button} onClick={() => void run({ type: 'resetPassword', email })}>忘记密码</button>
          </div>
        </form>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button type="button" disabled={busy} style={{ ...button, display: 'inline-flex', alignItems: 'center', gap: 10, minHeight: 40, padding: '10px 12px', background: '#fff', color: '#1f1f1f', border: '1px solid #747775' }} onClick={() => void run({ type: 'oauth', provider: 'google.com' })}><img src={googleIcon} alt="" width={20} height={20} />通过 Google 登录</button>
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'oauth', provider: 'apple.com' })}>通过 Apple 登录</button>
        </div>
        <span style={{ color: t.text3, fontSize: 12 }}>Google 和 Apple 登录将在系统浏览器中完成。</span>
      </>}
      <a href="https://lingxios.net/privacy" target="_blank" rel="noreferrer" style={{ color: t.text3, fontSize: 12 }}>隐私政策 ↗</a>
      {busy && <div role="status">正在处理… <button type="button" style={ghostButtonStyle(t)} onClick={() => void run({ type: 'cancel' })}>取消浏览器登录</button></div>}
      {(state?.error || message) && <p role={state?.error ? 'alert' : 'status'} style={{ margin: 0 }}>{state?.error || message}</p>}
      {state?.persistence === 'session' && <small style={{ color: t.text3 }}>此设备的安全存储暂不可用，关闭应用后需重新登录。</small>}
    </div>
  </Card>;
}
