import { useMemo, useState } from 'react';
import type { StructuredDiffDto } from '@lingxi/bridge-client';
import type { Locale } from '../utils/locale';
import { Markdown } from './markdown';
import { imageRefOf, itemsWithExtras, type ExtraRow, type PromptMeta, type Transcript } from './sessionModel';
import { transcriptRows, toolDisplayHeader, toolHasBody, toolTruncationNotice, type RunItem, type ToolRunItem, type TranscriptRow } from './reuse';
import { t } from './strings';
import { displayText } from './text';

interface TranscriptViewProps {
  locale: Locale;
  transcript: Transcript;
  running: boolean;
  /** This browser's device name, to tell its own prompts from other devices'. */
  deviceName: string;
  fetchImage(ref: string): Promise<string>;
}

function describeExtra(locale: Locale, extra: ExtraRow): string {
  if (extra.kind === 'omitted') return extra.detail ? t(locale, 'omittedKind', { kind: extra.detail }) : t(locale, 'omittedNote');
  return extra.detail ? t(locale, 'computerAccessNote', { apps: extra.detail }) : t(locale, 'computerAccessNoteNoApps');
}

function originLabel(locale: Locale, meta: PromptMeta, thisDevice: string): string {
  if (meta.origin === 'desktop') return t(locale, 'fromDesktop');
  if (meta.origin === 'scheduled') return t(locale, 'fromScheduled');
  if (meta.deviceName && meta.deviceName === thisDevice) return t(locale, 'fromThisDevice');
  return meta.deviceName ? t(locale, 'fromDevice', { name: meta.deviceName }) : t(locale, 'fromDevice', { name: '?' });
}

/** The conversation as rows: the reducer's items, grouped the way the desktop groups them. */
export function TranscriptView({ locale, transcript, running, deviceName, fetchImage }: TranscriptViewProps) {
  const rows = useMemo(
    () => transcriptRows(itemsWithExtras(transcript.conversation.items, transcript.extras, (extra) => describeExtra(locale, extra)), running),
    [transcript, running, locale],
  );
  return (
    <div className="rc-transcript">
      {rows.map((row) => (
        <Row key={row.id} locale={locale} row={row} transcript={transcript} deviceName={deviceName} fetchImage={fetchImage} />
      ))}
    </div>
  );
}

function Row({ locale, row, transcript, deviceName, fetchImage }: { locale: Locale; row: TranscriptRow; transcript: Transcript; deviceName: string; fetchImage(ref: string): Promise<string> }) {
  switch (row.type) {
    case 'tool-group':
      return (
        <div className="rc-tools">
          {row.tools.map((tool) => (
            <ToolCard key={tool.id} locale={locale} tool={tool} trimmed={transcript.truncatedTools.has(tool.id)} />
          ))}
        </div>
      );
    case 'narration':
      return <Narration locale={locale} item={row} meta={transcript.promptMeta.get(row.id)} deviceName={deviceName} fetchImage={fetchImage} />;
    case 'thinking':
      return <p className="rc-thinking" role="status">{t(locale, 'thinking')}</p>;
    case 'meta':
      return row.dur || row.files?.length ? (
        <p className="rc-meta">
          {displayText(row.dur, 120)}
          {row.files?.length ? ` · ${row.files.length} file${row.files.length === 1 ? '' : 's'}` : ''}
        </p>
      ) : null;
    case 'compaction':
      return <p className="rc-note">{displayText(row.detail ?? row.status, 120)}</p>;
    case 'command':
      return <pre className="rc-code rc-command">{row.output}</pre>;
    default:
      return null;
  }
}

function Narration({ locale, item, meta, deviceName, fetchImage }: { locale: Locale; item: Extract<RunItem, { type: 'narration' }>; meta: PromptMeta | undefined; deviceName: string; fetchImage(ref: string): Promise<string> }) {
  if (item.role === 'user') {
    return (
      <div className="rc-user">
        {meta ? <span className="rc-origin">{originLabel(locale, meta, deviceName)}</span> : null}
        <div className="rc-bubble">
          {item.text ? <p className="rc-plain">{item.text}</p> : null}
          <Images images={item.images ?? []} locale={locale} fetchImage={fetchImage} />
        </div>
      </div>
    );
  }
  if (item.role === 'assistant') {
    return (
      <div className="rc-assistant">
        <Markdown text={item.text} />
      </div>
    );
  }
  return <p className={`rc-note${item.tone === 'danger' ? ' rc-danger' : ''}`}>{item.text}</p>;
}

function Images({ images, locale, fetchImage }: { images: readonly { media_type: string; url: string }[]; locale: Locale; fetchImage(ref: string): Promise<string> }) {
  if (images.length === 0) return null;
  return (
    <div className="rc-images">
      {images.map((image, index) => (
        <RemoteImage key={`${index}:${image.url.slice(0, 64)}`} url={image.url} locale={locale} fetchImage={fetchImage} />
      ))}
    </div>
  );
}

/** A `data:` image shows at once; anything else is a reference to ask the desktop for, only when tapped. */
function RemoteImage({ url, locale, fetchImage }: { url: string; locale: Locale; fetchImage(ref: string): Promise<string> }) {
  const ref = imageRefOf(url);
  const [state, setState] = useState<{ status: 'idle' | 'loading' | 'failed' } | { status: 'ready'; src: string }>({ status: 'idle' });
  if (ref === undefined) return <img className="rc-image" src={url} alt={t(locale, 'imageAlt')} />;
  if (state.status === 'ready') return <img className="rc-image" src={state.src} alt={t(locale, 'imageAlt')} />;
  const load = (): void => {
    setState({ status: 'loading' });
    fetchImage(ref).then((src) => setState({ status: 'ready', src }), () => setState({ status: 'failed' }));
  };
  return (
    <button type="button" className="rc-image-ref" onClick={load} disabled={state.status === 'loading'}>
      {state.status === 'loading' ? t(locale, 'imageLoading') : state.status === 'failed' ? t(locale, 'imageFailed') : t(locale, 'imageLoad')}
    </button>
  );
}

function ToolCard({ locale, tool, trimmed }: { locale: Locale; tool: ToolRunItem; trimmed: boolean }) {
  const [open, setOpen] = useState(false);
  const header = toolDisplayHeader(tool);
  const body = toolHasBody(tool);
  const notice = toolTruncationNotice(tool);
  return (
    <div className={`rc-tool rc-tool-${tool.status}`}>
      <button type="button" className="rc-tool-head" onClick={() => setOpen((value) => !value)} disabled={!body} aria-expanded={body ? open : undefined}>
        <span className="rc-tool-title">{displayText(header.title || tool.tool, 200)}</span>
        {tool.status === 'running' ? <span className="rc-tool-state">{t(locale, 'toolRunning')}</span> : null}
        {tool.status === 'error' ? <span className="rc-tool-state rc-danger">{t(locale, 'toolFailed')}</span> : null}
        {body ? <span className="rc-tool-toggle">{open ? t(locale, 'hideOutput') : t(locale, 'showOutput')}</span> : null}
      </button>
      {tool.result?.headline ? <p className="rc-tool-headline">{displayText(tool.result.headline, 300)}</p> : null}
      {open && tool.result?.diff ? <Diff diff={tool.result.diff} /> : null}
      {open && tool.result?.body ? <pre className="rc-code">{tool.result.body}</pre> : null}
      {open && notice ? <p className="rc-note">{notice}</p> : null}
      {trimmed ? <p className="rc-note">{t(locale, 'truncatedTool')}</p> : null}
    </div>
  );
}

function Diff({ diff }: { diff: StructuredDiffDto }) {
  return (
    <pre className="rc-code rc-diff">
      {diff.file_path ? <span className="rc-diff-file">{displayText(diff.file_path, 300)}{'\n'}</span> : null}
      {diff.rows.map((row, index) => (
        <span key={index} className={`rc-diff-${row.kind}`}>
          {row.kind === 'add' ? '+' : row.kind === 'remove' ? '-' : ' '}
          {row.segments.map((segment, i) => (
            <span key={i} className={segment.emph ? 'rc-diff-emph' : undefined}>{segment.text}</span>
          ))}
          {'\n'}
        </span>
      ))}
    </pre>
  );
}
