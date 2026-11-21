import type { DatasetToolProposal, DatasetTurn, DatasetTurnRequest } from './client';
import { errorText } from './errors';

interface DatasetTurnControlsProps {
  zh: boolean;
  id: string;
  busy: boolean;
  progress?: { turn?: DatasetTurn; error?: string };
  onRequest(request: DatasetTurnRequest): void;
}

function proposalTitle(name: DatasetToolProposal['name'], zh: boolean): string {
  switch (name) {
    case 'dataset_create': return zh ? '创建数据集' : 'Create Dataset';
    case 'dataset_put': return zh ? '写入记录' : 'Write record';
    case 'dataset_latest': return zh ? '读取记录并发送给模型' : 'Read record and send to the model';
  }
}

export function DatasetTurnControls({ zh, id, busy, progress, onRequest }: DatasetTurnControlsProps) {
  const t = (en: string, cn: string): string => zh ? cn : en;
  const turn = progress?.turn;
  const approval = turn?.state === 'approval' ? turn : undefined;
  return <div className="agent-dataset-controls">
    <p role="status">{approval ? t('Review the proposed Dataset actions.', '请检查待批准的数据操作。')
      : busy ? '' : t('Dataset turn can be resumed.', '可以继续上次的数据操作。')}</p>
    {progress?.error ? <p role="alert">{errorText(progress.error, zh)}</p> : null}
    {approval ? <>
      <ul className="agent-dataset-actions">
        {approval.calls.map(call => <li key={call.id}>
          <strong>{proposalTitle(call.name, zh)}</strong>
          <pre>{JSON.stringify(call.arguments, null, 2)}</pre>
        </li>)}
      </ul>
      <div className="agent-dataset-buttons">
        <button type="button" className="chat-button" disabled={busy}
          onClick={() => onRequest({ action: 'approve', id, approval: approval.approval })}>{t('Approve', '批准')}</button>
        <button type="button" className="chat-button chat-button-quiet" disabled={busy}
          onClick={() => onRequest({ action: 'deny', id, approval: approval.approval })}>{t('Deny', '拒绝')}</button>
      </div>
    </> : <div className="agent-dataset-buttons">
      <button type="button" className="chat-button" disabled={busy}
        onClick={() => onRequest({ action: 'get', id })}>{t('Resume', '继续')}</button>
      <button type="button" className="chat-button chat-button-quiet" disabled={busy || turn?.state === 'executing'}
        onClick={() => onRequest({ action: 'cancel', id })}>{t('Cancel', '取消')}</button>
    </div>}
  </div>;
}
