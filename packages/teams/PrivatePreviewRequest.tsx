import { useEffect, useId, useRef, useState, type FormEvent, type ReactElement } from 'react';
import type { TeamsApi } from './index.js';
import type { PrivateSourceOffer, TeamDataScope } from './lingxi-data.js';

export function PrivatePreviewRequest({ api, scope, locale, request }: {
  api: TeamsApi; scope: TeamDataScope; locale: string; request(value: PrivateSourceOffer): Promise<boolean>;
}): ReactElement | null {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const [offers, setOffers] = useState<PrivateSourceOffer[]>([]);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState(false);
  const { teamId, generation, conversationId } = scope;
  const zh = locale.toLowerCase().startsWith('zh');
  const title = zh ? '请求私有预览' : 'Request private preview';

  useEffect(() => {
    if (!api.data) return;
    let current = true;
    void api.data<{ sources: PrivateSourceOffer[] }>({
      scope: { kind: 'team', teamId, generation, conversationId }, command: { action: 'privateSourceOffers' },
    }).then(result => { if (current) setOffers(result.sources); }, () => { if (current) setOffers([]); });
    return () => { current = false; };
  }, [api, teamId, generation, conversationId]);

  if (!offers.length) return null;

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const index = Number(new FormData(event.currentTarget).get('source'));
    const offer = offers[index];
    if (!offer) return;
    setWorking(true); setError(false);
    void request(offer).then(sent => {
      if (sent) dialog.current?.close();
      else setError(true);
    }, () => setError(true)).finally(() => setWorking(false));
  }

  return <>
    <button type="button" className="tw-head-action" onClick={() => { setError(false); dialog.current?.showModal(); }}>{title}</button>
    <dialog className="tw-dialog tw-data-record" ref={dialog} aria-labelledby={titleId}>
      <div className="tw-dialog-panel tw-data-workspace">
        <h2 id={titleId}>{title}</h2>
        <form method="post" onSubmit={submit}>
          <label>{zh ? '数据源与处理 Bot' : 'Source and processing Bot'}<select name="source" disabled={working}>
            {offers.map((offer, index) => <option key={offer.source + ':' + offer.sharedBotId} value={index}>{offer.name} · {offer.botName}</option>)}
          </select></label>
          <p>{zh ? '选定 Bot 的宿主和模型会处理已授权字段。共享会话只显示请求状态。' :
            'The selected Bot host and model process the permitted fields. Shared Chat displays the request status.'}</p>
          {error && <p role="alert">{zh ? '未能提交请求。' : 'The request could not be submitted.'}</p>}
          <button type="submit" disabled={working}>{title}</button>
        </form>
        <form method="dialog"><button type="submit" disabled={working}>{zh ? '取消' : 'Cancel'}</button></form>
      </div>
    </dialog>
  </>;
}
