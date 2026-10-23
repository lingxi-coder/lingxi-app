import { useState } from 'react';
import { buildDeepLink, parseDeepLink } from '../../../../packages/deep-links';
import { remoteSessionRoute } from '../app/router';
import type { Locale } from '../utils/locale';

export function OpenLinkPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  const [notice, setNotice] = useState('');
  let scheme = '';
  let canonical = '';
  let viewer = '';
  try {
    // Parse the production authority, keeping the current path/query/fragment.
    const target = parseDeepLink(`https://lingxios.net${location.pathname}${location.search}${location.hash}`);
    scheme = buildDeepLink(target, 'scheme');
    canonical = buildDeepLink(target);
    const [kind, hostId, sessionId] = target.route.split('/');
    // A full page load, so /remote starts under its own strict policy.
    if (kind === 'remote') viewer = remoteSessionRoute(hostId!, sessionId!);
  } catch { /* Invalid links never become executable hrefs. */ }
  return <main className="page-wrap"><header className="page-hero compact"><span className="eyebrow">LingXi</span><h1>{scheme ? (zh ? '在灵犀中继续' : 'Continue in LingXi') : (zh ? '此链接不可用' : 'This link is unavailable')}</h1><p>{scheme ? (zh ? '在这台设备上打开对应内容。消息、命令和配置会在应用内等待你确认。' : 'Open this destination on your device. Review drafts, commands and configurations in the app.') : (zh ? '请检查链接是否完整，或重新复制链接。' : 'Check the link or copy it again.')}</p></header>{scheme && <div className="landing-actions"><a className="button primary" href={scheme}>{zh ? '在灵犀中打开' : 'Open LingXi'}</a>{viewer && <a className="button secondary" href={viewer}>{zh ? '在此浏览器中遥控' : 'Control from this browser'}</a>}<button className="button secondary" onClick={() => { void navigator.clipboard.writeText(canonical).then(() => setNotice(zh ? '链接已复制' : 'Link copied'), () => setNotice(zh ? '无法复制，请复制浏览器地址' : 'Copy the browser address instead')); }}>{zh ? '复制链接' : 'Copy link'}</button></div>}<p role="status">{notice}</p><p>{zh ? '尚未安装？查看各平台的开放状态。安装后重新打开原链接。' : 'Not installed? Check platform availability, then reopen this link after installation.'}</p><a className="landing-text-link" href="/download">{zh ? '查看下载' : 'View downloads'}</a></main>;
}
