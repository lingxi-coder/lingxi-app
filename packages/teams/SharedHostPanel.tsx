import { useEffect, useState } from 'react';
import type { SharedBotCard, SharedHostBot, SharedHostStatus } from './execution.js';
import type { TeamsApi } from './index.js';

/**
 * This Mac as the host of the Bots its owner publishes here: which of them it answers for, what stands in the way, and the two
 * switches that keep it serving while the owner is away (the same ones as on the Bots page). Desktop only. The state is the host's
 * and the system's, so nothing is kept: it is read again whenever the cards change and after each change made here.
 */
export function SharedHostPanel({ api, teamId, bots, zh, busy, attempt }: {
  api: TeamsApi; teamId: string; /** The published Bots of this team or friendship that the person maintains. */ bots: SharedBotCard[]; zh: boolean; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [status, setStatus] = useState<SharedHostStatus>(), [reads, setReads] = useState(0);
  const cards = bots.map(bot => `${bot.sharedBotId}:${bot.revision}:${bot.ready}`).join('|');
  useEffect(() => {
    let current = true;
    void api.hostStatus!().then(value => { if (current) setStatus(value); }, () => { if (current) setStatus(undefined); });
    return () => { current = false; };
  }, [api, cards, reads]);
  if (!status || !bots.length) return null;
  const served = (bot: SharedBotCard) => status.bots.find(value => value.teamId === teamId && value.sharedBotId === bot.sharedBotId);
  const line = (host: SharedHostBot | undefined) => {
    if (!host) return t('is published from another computer. This Mac does not answer for it.', '由另一台电脑发布，这台 Mac 不为它作答。');
    if (host.state === 'ready') return host.running ? t(`is answering from this Mac · ${host.running} in progress`, `由这台 Mac 作答 · ${host.running} 个请求进行中`) : t('is answering from this Mac.', '由这台 Mac 作答。');
    if (host.state === 'checking') return t('is being checked…', '正在检查…');
    if (host.state === 'unreachable') return t('cannot answer: this Mac cannot reach LingXi. It tries again by itself.', '无法作答：这台 Mac 连接不上灵犀，稍后会自动重试。');
    if (host.state === 'stopped') return t('is no longer served by this Mac. Publish it again here to answer from this Mac.', '已不再由这台 Mac 提供服务。在这里重新发布后才由这台 Mac 作答。');
    return host.reason === 'bot' ? t('cannot answer: the local Bot it was published from is no longer on this Mac. Publish it again from a Bot you have.', '无法作答：发布它所用的本机 Bot 已不在这台 Mac 上。请用现有的 Bot 重新发布。')
      : host.reason === 'runtime' ? t('cannot answer: this copy of LingXi cannot run it. Update LingXi or install it again.', '无法作答：当前安装的灵犀无法运行它。请更新或重新安装灵犀。')
      : t('cannot answer: its model account is not available on this Mac. Check the account in Settings.', '无法作答：它的模型账号在这台 Mac 上不可用。请在设置中检查该账号。');
  };
  const here = bots.some(bot => served(bot)), { openAtLogin, keepAwake, onBattery } = status;
  const change = (value: { openAtLogin?: boolean; keepAwake?: boolean }) => void attempt(async () => { try { await api.setHostBackground!(value); } finally { setReads(count => count + 1); } });
  return <section className="team-bot-host" aria-label={t('This Mac', '这台 Mac')}>
    <h4>{t('This Mac', '这台 Mac')}</h4>
    <ul>{bots.map(bot => { const host = served(bot); return <li key={bot.sharedBotId} data-state={host?.state ?? 'elsewhere'}><b>{bot.name}</b> {line(host)}</li>; })}</ul>
    {here ? <>
      <p className="teams-hint">{t('This Mac keeps answering after you close the window. It stops when you quit LingXi or sign out, and while the Mac is asleep or offline. People then see the assistant as unavailable, and it takes no new requests until it is back.', '关闭窗口后，这台 Mac 仍会继续作答。退出灵犀或退出账号后停止；Mac 睡眠或断网期间也无法作答。此时成员会看到助手不可用，恢复之前不接受新请求。')}</p>
      <label className="team-party"><input type="checkbox" checked={openAtLogin.enabled} disabled={busy || !openAtLogin.available || !api.setHostBackground} onChange={event => change({ openAtLogin: event.target.checked })} />{t('Open LingXi in the background when you log in', '登录时在后台打开灵犀')}</label>
      {!openAtLogin.available ? <p className="teams-hint">{t('Available in the installed app.', '仅在已安装的应用中可用。')}</p>
        : openAtLogin.needsApproval ? <p role="alert">{t('Allow LingXi in System Settings → General → Login Items.', '请在“系统设置 → 通用 → 登录项”中允许灵犀。')} {api.openLoginItems ? <button type="button" disabled={busy} onClick={() => void attempt(() => api.openLoginItems!())}>{t('Open Login Items', '打开登录项')}</button> : null}</p>
        : !openAtLogin.enabled ? <p role="alert">{t('After this Mac restarts, the assistant stays unavailable until you open LingXi.', '这台 Mac 重新启动后，在你打开灵犀之前助手都不可用。')}</p> : null}
      <label className="team-party"><input type="checkbox" checked={keepAwake} disabled={busy || !api.setHostBackground} onChange={event => change({ keepAwake: event.target.checked })} />{t('Keep this Mac awake while Bots are working', 'Bot 工作时让这台 Mac 保持唤醒')}</label>
      {!keepAwake ? <p role="alert">{t('This Mac may go to sleep when it is idle. The assistant is unavailable while it sleeps.', '这台 Mac 空闲时可能进入睡眠，睡眠期间助手不可用。')}</p>
        : onBattery ? <p role="alert">{t('On battery, this Mac stays awake only while a request is being answered. Plug it in to keep the assistant available.', '使用电池时，这台 Mac 只在处理请求期间保持唤醒。接通电源才能让助手持续可用。')}</p>
        : <p className="teams-hint">{t('While plugged in, this Mac stays awake for as long as an assistant you share is online. Closing the lid still puts it to sleep.', '接通电源时，只要你共享的助手在线，这台 Mac 就保持唤醒。合上盖子仍会进入睡眠。')}</p>}
    </> : null}
  </section>;
}
