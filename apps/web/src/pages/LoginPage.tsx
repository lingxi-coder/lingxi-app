import { FormEvent, useState } from 'react';
import { navigateTo } from '../app/router';
import { IconCheck, IconGlobe, IconSpark, IconTerminal } from '../components/Icons';
import { Locale, Region, pickLocaleText } from '../utils/locale';

export function LoginPage({ locale, region, onRegionChange }: { locale: Locale; region: Region; onRegionChange: (region: Region) => void }) {
  const [method, setMethod] = useState<'email' | 'phone'>(region === 'china' ? 'phone' : 'email');
  const [sent, setSent] = useState(false);
  const t = (en: string, zh: string) => pickLocaleText(locale, { en, zh });
  const submit = (event: FormEvent<HTMLFormElement>) => { event.preventDefault(); setSent(true); window.setTimeout(() => navigateTo('/console'), 700); };
  return (
    <div className="login-page">
      <section className="login-story">
        <span className="eyebrow"><IconSpark width={15} height={15} /> LingXi continuity · UI prototype</span>
        <h1>{t('Your work should remember where you left it.', '你的工作，应该记得你停在哪里。')}</h1>
        <p>{t('The intended account experience keeps tasks, approvals, and API activity attached to one personal identity. No account backend is connected in this prototype.', '目标账号体验会让任务、审批与 API 活动归属于同一个个人身份；此原型尚未接入账号后端。')}</p>
        <div className="login-terminal"><div><IconTerminal width={16} height={16} /> task_08 / checkout</div><code>context restored <b>100%</b><br />approval received <b>mobile</b><br />next action <b>run tests</b></code></div>
        <ul><li><IconCheck width={15} height={15} /> {t('Passwordless by default', '默认无密码登录')}</li><li><IconCheck width={15} height={15} /> {t('Region-aware, always switchable', '按地区推荐，始终可以切换')}</li><li><IconCheck width={15} height={15} /> {t('Individual account v1', '首版个人账号')}</li></ul>
      </section>
      <section className="login-card">
        <div className="login-region"><span><IconGlobe width={16} height={16} /> {t('Sign-in region', '登录地区')}</span><div className="segmented-control"><button type="button" className={region === 'global' ? 'active' : ''} onClick={() => { onRegionChange('global'); setMethod('email'); }}>Global</button><button type="button" className={region === 'china' ? 'active' : ''} onClick={() => { onRegionChange('china'); setMethod('phone'); }}>中国</button></div></div>
        <span className="eyebrow">Developer Console</span><h2>{t('Welcome to LingXi', '欢迎来到 LingXi')}</h2><p>{region === 'china' ? t('Use phone, WeChat, or email to continue.', '使用手机号、微信或邮箱继续。') : t('Use email or GitHub to continue.', '使用邮箱或 GitHub 继续。')}</p>
        <div className="auth-methods">
          {region === 'china' ? <><button type="button" className={method === 'phone' ? 'active' : ''} onClick={() => setMethod('phone')}>{t('Phone code', '手机验证码')}</button><button type="button" disabled title={t('Unavailable in prototype', '原型中不可用')}>WeChat · {t('Unavailable', '不可用')}</button><button type="button" className={method === 'email' ? 'active' : ''} onClick={() => setMethod('email')}>{t('Email code', '邮箱验证码')}</button></> : <><button type="button" className={method === 'email' ? 'active' : ''} onClick={() => setMethod('email')}>{t('Email code', '邮箱验证码')}</button><button type="button" disabled title={t('Unavailable in prototype', '原型中不可用')}>GitHub · {t('Unavailable', '不可用')}</button></>}
        </div>
        <div className="auth-divider"><span>{t('or continue with a code', '或使用验证码')}</span></div>
        <form className="login-form" onSubmit={submit}>
          <label>{method === 'phone' ? t('Phone number', '手机号') : t('Email address', '邮箱地址')}<input required type={method === 'phone' ? 'tel' : 'email'} placeholder={method === 'phone' ? '+86 138 0000 0000' : 'you@company.com'} /></label>
          <button type="submit" className="button primary">{sent ? t('Opening console…', '正在进入控制台…') : t('Send code', '发送验证码')}</button>
        </form>
        <small>{t('Prototype sign-in: no message is sent and no account is created.', '原型登录：不会发送消息，也不会创建真实账号。')}</small>
      </section>
    </div>
  );
}
