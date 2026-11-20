import type { ChatTarget } from '../app/router';
import { appLinkForChat } from '../pages/openLinkTarget';
import type { Locale } from '../utils/locale';

/** "Open in the LingXi app" for a shared link that continued in the browser, where the app may still be installed. */
export function OpenInAppBanner({ target, prompt, locale, onClose }: { target: ChatTarget; prompt?: string; locale: Locale; onClose(): void }) {
  const zh = locale === 'zh';
  const href = appLinkForChat(target, prompt);
  if (!href) return null;
  return <div className="chat-app-banner" role="region" aria-label={zh ? '在灵犀 App 中打开' : 'Open in the LingXi app'}>
    <span>{zh ? '已安装灵犀 App？在 App 中继续这个对话。' : 'Have the LingXi app? Continue this conversation there.'}</span>
    <a className="chat-button" href={href}>{zh ? '在灵犀 App 中打开' : 'Open in the LingXi app'}</a>
    <button type="button" className="chat-app-banner-close" aria-label={zh ? '关闭' : 'Dismiss'} onClick={onClose}>×</button>
  </div>;
}
