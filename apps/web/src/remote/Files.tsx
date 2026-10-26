import { useEffect, useRef, useState } from 'react';
import type { FileReceipt } from '@lingxi/bridge-client/remote';
import type { Locale } from '../utils/locale';
import type { RemoteClient } from './remoteClient';

/** Original bytes, independent of the compressed image picker. */
export function Files({ client, session, locale, disabled, onAttach }: {
  client: RemoteClient; session: string; locale: Locale; disabled: boolean; onAttach(file: FileReceipt): void;
}) {
  const zh = locale === 'zh';
  const [files, setFiles] = useState<FileReceipt[]>([]);
  const [selected, setSelected] = useState<File>();
  const [progress, setProgress] = useState('');
  const [problem, setProblem] = useState('');
  const [busy, setBusy] = useState(false);
  const abort = useRef<AbortController>();
  const input = useRef<HTMLInputElement>(null);
  const generation = useRef(0);
  useEffect(() => { setFiles([]); setSelected(undefined); }, [client, session]);
  useEffect(() => {
    const mine = ++generation.current;
    setBusy(false);
    if (!disabled) void client.listFiles(session).then(result => {
      if (generation.current === mine) setFiles(current => [...new Map([...result, ...current].map(file => [file.ref, file])).values()]);
    }).catch(() => undefined);
    return () => { ++generation.current; abort.current?.abort(); abort.current = undefined; };
  }, [client, session, disabled]);
  const start = () => {
    if (disabled || abort.current) return;
    const controller = new AbortController(); abort.current = controller;
    const mine = generation.current;
    const current = () => generation.current === mine && abort.current === controller;
    setBusy(true); setProblem('');
    return {
      controller, current,
      report: (offset: number, size: number) => {
        if (current() && !controller.signal.aborted) setProgress(`${size ? Math.floor(offset / size * 100) : 100}% · ${offset.toLocaleString()} / ${size.toLocaleString()} B`);
      },
      finish: () => { if (current()) { setBusy(false); abort.current = undefined; } },
    };
  };
  const upload = async (file: File): Promise<void> => {
    const operation = start(); if (!operation) return;
    setProgress(zh ? '校验文件…' : 'Hashing file…'); setSelected(file);
    try {
      const result = await client.uploadFile(session, file, operation.report, operation.controller.signal);
      if (!operation.current() || operation.controller.signal.aborted) return;
      setFiles(current => [...current.filter(f => f.ref !== result.ref), result]); setSelected(undefined); onAttach(result);
    } catch (error) { if (operation.current()) setProblem(error instanceof Error ? error.message : String(error)); }
    finally { operation.finish(); }
  };
  const download = async (file: FileReceipt): Promise<void> => {
    const operation = start(); if (!operation) return;
    setProgress('');
    try {
      const blob = await client.downloadFile(file, operation.report, operation.controller.signal);
      if (!operation.current() || operation.controller.signal.aborted) return;
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a'); link.href = url; link.download = file.name; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    } catch (error) { if (operation.current()) setProblem(error instanceof Error ? error.message : String(error)); }
    finally { operation.finish(); }
  };
  return <details className="rc-files">
    <summary>{zh ? '文件' : 'Files'} ({files.length})</summary>
    <button type="button" disabled={disabled || busy} onClick={() => input.current?.click()}>{zh ? '上传原文件' : 'Upload file'}</button>
    <input type="file" ref={input} hidden onChange={event => {
      const file = event.target.files?.[0]; event.target.value = ''; if (file) void upload(file);
    }} />
    {busy && abort.current ? <button type="button" onClick={() => abort.current?.abort()}>{zh ? '取消传输' : 'Cancel transfer'}</button> : null}
    {!busy && selected ? <button type="button" disabled={disabled} onClick={() => void upload(selected)}>{zh ? '重试 / 续传' : 'Retry / resume'}</button> : null}
    {progress ? <p role="status">{progress}</p> : null}
    {problem ? <p role="alert">{problem}</p> : null}
    <ul>{files.map(file => <li key={file.ref}>
      {file.name} · {file.size.toLocaleString()} B{' '}
      <button type="button" disabled={disabled || busy} onClick={() => onAttach(file)}>{zh ? '附到消息' : 'Attach'}</button>{' '}
      <button type="button" disabled={disabled || busy} onClick={() => void download(file)}>{zh ? '下载' : 'Download'}</button>
    </li>)}</ul>
    {disabled ? <p className="rc-note">{zh ? '文件传输需要 WebRTC 连接。' : 'File transfer requires a WebRTC connection.'}</p> : null}
  </details>;
}
