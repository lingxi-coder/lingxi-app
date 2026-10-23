import { useRef, useState } from 'react';
import type { ImageRefDto } from '@lingxi/bridge-client';
import type { Locale } from '../utils/locale';
import { ImageRejected, shrinkImage } from './imageShrink';
import { MAX_IMAGE_ATTACHMENTS } from './reuse';
import { t, type StringKey } from './strings';

interface ComposerProps {
  locale: Locale;
  /** A turn is running: text only, and the stop button shows. */
  running: boolean;
  disabled: boolean;
  hasFiles?: boolean;
  onSend(text: string, images: ImageRefDto[]): void;
  onStop(): void;
}

const IMAGE_ERRORS: Record<ImageRejected['reason'], StringKey> = {
  type: 'imageTypeError',
  decode: 'imageDecodeError',
  size: 'imageSizeError',
};

export function Composer({ locale, running, disabled, hasFiles, onSend, onStop }: ComposerProps) {
  const [text, setText] = useState('');
  const [images, setImages] = useState<ImageRefDto[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [shrinking, setShrinking] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const canSend = !disabled && !shrinking && (text.trim().length > 0 || hasFiles || (!running && images.length > 0));

  const send = (): void => {
    if (!canSend) return;
    onSend(text, running ? [] : images);
    setText('');
    setImages([]);
    setProblem(null);
  };

  const pick = async (files: FileList | null): Promise<void> => {
    if (files === null || files.length === 0) return;
    setProblem(null);
    setShrinking(true);
    try {
      const added: ImageRefDto[] = [];
      for (const file of Array.from(files)) {
        if (images.length + added.length >= MAX_IMAGE_ATTACHMENTS) {
          setProblem(t(locale, 'imageLimit', { count: MAX_IMAGE_ATTACHMENTS }));
          break;
        }
        try {
          added.push(await shrinkImage(file));
        } catch (error) {
          setProblem(t(locale, error instanceof ImageRejected ? IMAGE_ERRORS[error.reason] : 'imageDecodeError'));
        }
      }
      if (added.length > 0) setImages((current) => [...current, ...added]);
    } finally {
      setShrinking(false);
      if (fileInput.current) fileInput.current.value = '';
    }
  };

  return (
    <form
      className="rc-composer"
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      {images.length > 0 ? (
        <ul className="rc-attachments">
          {images.map((image, index) => (
            <li key={index}>
              <img src={`data:${image.media_type};base64,${image.base64}`} alt={t(locale, 'imageAlt')} />
              <button type="button" aria-label={t(locale, 'removeImage')} onClick={() => setImages((current) => current.filter((_, i) => i !== index))}>
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {problem ? <p className="rc-note rc-danger" role="alert">{problem}</p> : null}
      {running && images.length === 0 ? <p className="rc-note">{t(locale, 'imagesOnlyWhenIdle')}</p> : null}
      <div className="rc-composer-row">
        <button
          type="button"
          className="rc-icon-button"
          aria-label={t(locale, 'attachImage')}
          disabled={disabled || running || shrinking || images.length >= MAX_IMAGE_ATTACHMENTS}
          onClick={() => fileInput.current?.click()}
        >
          +
        </button>
        <input ref={fileInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden onChange={(event) => void pick(event.target.files)} />
        <textarea
          value={text}
          rows={1}
          placeholder={t(locale, running ? 'composerPlaceholderRunning' : 'composerPlaceholder')}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            // Enter adds a line on a phone keyboard; Ctrl/Cmd+Enter sends.
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              send();
            }
          }}
        />
        {running ? (
          <button type="button" className="button secondary" onClick={onStop}>
            {t(locale, 'stop')}
          </button>
        ) : null}
        <button type="submit" className="button primary" disabled={!canSend}>
          {t(locale, 'send')}
        </button>
      </div>
    </form>
  );
}
