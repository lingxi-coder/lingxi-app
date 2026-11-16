import { useEffect, useRef, useState, type ReactElement } from 'react';
import { downloadAttachment, fileSize, isImage, type DraftFile, type MessageAttachment } from './attachments.js';
import { messageErrorText } from './errors.js';
import type { ExecutionScope } from './execution.js';
import type { MessageApi } from './messages.js';
import { Dialog, Icon } from './ui';

type Api = Pick<MessageApi, 'messages'>;
/** Pictures up to this size are shown without being asked for; a larger one waits for a click. */
const AUTO_PREVIEW_BYTES = 3 * 1024 * 1024;
// Bytes already read on this page, for the membership they were read under. A file is the same bytes for as long as it exists, and
// a file that was taken back is never looked up here: its message no longer offers it.
const CACHE_BYTES = 32 * 1024 * 1024, cache = new Map<string, Uint8Array>();
const cacheKey = (scope: ExecutionScope, file: MessageAttachment) => `${scope.teamId}:${scope.generation}:${file.attachmentId}`;
async function bytesOf(api: Api, scope: ExecutionScope, file: MessageAttachment): Promise<Uint8Array> {
  const key = cacheKey(scope, file), kept = cache.get(key);
  if (kept) return kept;
  const bytes = await downloadAttachment(api, scope, file);
  cache.set(key, bytes);
  let total = 0;
  for (const value of cache.values()) total += value.length;
  for (const old of cache.keys()) { if (total <= CACHE_BYTES || old === key) break; total -= cache.get(old)!.length; cache.delete(old); }
  return bytes;
}
/** Forget what this page read of a file (it was taken back). */
export const forgetAttachment = (scope: ExecutionScope, file: MessageAttachment): void => { cache.delete(cacheKey(scope, file)); };
const blobOf = (file: MessageAttachment, bytes: Uint8Array) => new Blob([bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer], { type: file.mime });
/** Hand a file to the browser to save under its own name. */
export function saveFile(file: MessageAttachment, bytes: Uint8Array): void {
  const url = URL.createObjectURL(blobOf(file, bytes)), link = document.createElement('a');
  link.href = url; link.download = file.name; link.rel = 'noopener';
  document.body.append(link); link.click(); link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}

/** The files of an unsent message: what each is, how far its upload is, and a way to take it out again. */
export function DraftFiles({ files, zh, onRemove }: { files: readonly DraftFile[]; zh: boolean; onRemove(uploadId: string): void }): ReactElement | null {
  const t = (en: string, cn: string) => zh ? cn : en, locale = zh ? 'zh-CN' : 'en-US';
  if (!files.length) return null;
  return <ul className="tw-files tw-draft-files" aria-label={t('Files to send', '待发送的文件')}>
    {files.map(file => <li key={file.uploadId} className="tw-file" data-state={file.state}>
      <Icon name={isImage(file) ? 'image' : 'file'} />
      <span className="tw-file-text"><b>{file.name}</b>
        <small role="status">{file.state === 'failed' ? messageErrorText(file.error ?? 'UPLOAD_FAILED', zh)
          : file.state === 'uploading' ? t(`Uploading… ${Math.floor(file.sent / file.byteLength * 100)}%`, `正在上传… ${Math.floor(file.sent / file.byteLength * 100)}%`)
          : fileSize(file.byteLength, locale)}</small></span>
      <button type="button" className="tw-icon-btn" aria-label={t(`Remove ${file.name}`, `移除 ${file.name}`)} onClick={() => onRemove(file.uploadId)}><Icon name="x" size={12} /></button>
    </li>)}
  </ul>;
}

/**
 * The files a message carries. A picture is shown in place; any file can be saved; plain text can be read here. Every look at a
 * file asks the service, which answers only while the viewer reads the conversation. The sender can take a file back.
 */
export function MessageFiles({ api, scope, files, zh, save = saveFile, onRevoke }: {
  api: Api; scope: ExecutionScope; files: readonly MessageAttachment[]; zh: boolean;
  /** Test seam: where a saved file goes. */ save?(file: MessageAttachment, bytes: Uint8Array): void;
  /** Present for the sender while they may still change the conversation. */ onRevoke?(file: MessageAttachment): void;
}): ReactElement {
  return <ul className="tw-files" aria-label={zh ? '文件' : 'Files'}>
    {files.map(file => <MessageFile key={file.attachmentId} api={api} scope={scope} file={file} zh={zh} save={save} onRevoke={onRevoke} />)}
  </ul>;
}

function MessageFile({ api, scope, file, zh, save, onRevoke }: { api: Api; scope: ExecutionScope; file: MessageAttachment; zh: boolean; save(file: MessageAttachment, bytes: Uint8Array): void; onRevoke?(file: MessageAttachment): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en, locale = zh ? 'zh-CN' : 'en-US';
  const picture = isImage(file) && !file.revoked;
  const [url, setUrl] = useState(''), [busy, setBusy] = useState(false), [error, setError] = useState(''), [open, setOpen] = useState(false), [text, setText] = useState<string>();
  const [wanted, setWanted] = useState(picture && file.byteLength <= AUTO_PREVIEW_BYTES);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const read = async <T,>(use: (bytes: Uint8Array) => T): Promise<T | undefined> => {
    setBusy(true); setError('');
    try { const bytes = await bytesOf(api, scope, file); return alive.current ? use(bytes) : undefined; }
    catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : 'SERVICE_UNAVAILABLE'); return undefined; }
    finally { if (alive.current) setBusy(false); }
  };
  // The picture itself, once wanted. Its object URL lives as long as this row shows it.
  useEffect(() => {
    if (!wanted || !picture) return;
    let made = '';
    void read(bytes => { made = URL.createObjectURL(blobOf(file, bytes)); setUrl(made); });
    return () => { if (made) URL.revokeObjectURL(made); setUrl(''); };
  // One file, one membership.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wanted, picture, file.attachmentId, scope.teamId, scope.generation]);
  useEffect(() => { if (file.revoked) forgetAttachment(scope, file); }, [file, scope]);

  if (file.revoked) return <li className="tw-file" data-state="revoked"><Icon name="file" /><span className="tw-file-text"><b>{file.name}</b><small>{t('Taken back by its sender', '已被发送者收回')}</small></span></li>;
  const size = fileSize(file.byteLength, locale);
  const actions = <>
    {file.mime === 'text/plain' ? <button type="button" disabled={busy} onClick={() => void read(bytes => { setText(new TextDecoder().decode(bytes.subarray(0, 256 * 1024))); setOpen(true); })}>{t('Open', '打开')}</button> : null}
    <button type="button" className="tw-icon-btn" disabled={busy} aria-label={t(`Save ${file.name}`, `保存 ${file.name}`)} title={t('Save', '保存')} onClick={() => void read(bytes => save(file, bytes))}><Icon name="download" /></button>
    {onRevoke ? <button type="button" className="tw-icon-btn tw-danger" aria-label={t(`Take back ${file.name}`, `收回 ${file.name}`)} title={t('Take back', '收回')} onClick={() => onRevoke(file)}><Icon name="trash" /></button> : null}
  </>;
  return <li className="tw-file" data-kind={picture ? 'image' : 'file'}>
    {picture && url ? <button type="button" className="tw-file-picture" aria-label={t(`View ${file.name}`, `查看 ${file.name}`)} onClick={() => setOpen(true)}><img src={url} alt={file.name} /></button>
      : picture && !wanted ? <button type="button" className="tw-file-picture tw-file-placeholder" onClick={() => setWanted(true)}><Icon name="image" size={20} />{t('Show picture', '显示图片')}</button>
      : <Icon name={picture ? 'image' : 'file'} />}
    <span className="tw-file-text"><b>{file.name}</b>
      <small role={error ? 'alert' : 'status'}>{error ? messageErrorText(error, zh) : busy ? t('Loading…', '正在加载…') : size}</small></span>
    <span className="tw-file-actions">{actions}</span>
    {open ? <Dialog title={file.name} onClose={() => setOpen(false)}>
      {picture ? <img className="tw-file-full" src={url} alt={file.name} /> : <pre className="tw-file-textview">{text}{file.byteLength > 256 * 1024 ? `\n${t('… Save the file to read the rest.', '… 保存文件以阅读其余内容。')}` : ''}</pre>}
    </Dialog> : null}
  </li>;
}
