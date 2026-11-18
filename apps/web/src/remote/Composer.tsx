import { useRef, useState } from 'react';
import { ComposerEditor, ComposerImageThumbnail, ComposerSurface, ComposerToolbar } from '../../../../packages/chat-ui/ComposerSurface';
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
  onSend(text: string, images: ImageRefDto[]): boolean;
  onStop(): void;
}

const IMAGE_ERRORS: Record<ImageRejected['reason'], StringKey> = {
  type: 'imageTypeError',
  decode: 'imageDecodeError',
  size: 'imageSizeError',
};

interface DraftImage {
  id: number;
  image: ImageRefDto;
}

function ImageAttachment({ image, locale, onRemove }: { image: ImageRefDto; locale: Locale; onRemove(): void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [open, setOpen] = useState(false);
  const src = `data:${image.media_type};base64,${image.base64}`;
  const previewLabel = t(locale, 'previewImage');
  return (
    <li className="composer-image-attachment">
      <button type="button" className="composer-image-preview" aria-label={previewLabel} aria-haspopup="dialog"
        onClick={() => {
          dialog.current?.showModal();
          setOpen(true);
        }}>
        <ComposerImageThumbnail src={src} />
      </button>
      <button type="button" className="composer-image-remove" aria-label={t(locale, 'removeImage')} onClick={onRemove}>
        <span aria-hidden="true">×</span>
      </button>
      <dialog ref={dialog} className="rc-image-preview" aria-label={previewLabel} onClose={() => setOpen(false)}>
        <button type="button" className="rc-image-preview-close" aria-label={t(locale, 'closeImagePreview')}
          onClick={() => dialog.current?.close()}>
          <span aria-hidden="true">×</span>
        </button>
        {open && <img src={src} alt={t(locale, 'imageAlt')} />}
      </dialog>
    </li>
  );
}

export function Composer({ locale, running, disabled, hasFiles, onSend, onStop }: ComposerProps) {
  const [text, setText] = useState('');
  const [images, setImages] = useState<DraftImage[]>([]);
  const nextImageId = useRef(0);
  const [problem, setProblem] = useState<string | null>(null);
  const [shrinking, setShrinking] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const canSend = !disabled && !shrinking && (text.trim().length > 0 || (!running && (hasFiles || images.length > 0)));

  const send = (): void => {
    if (!canSend) return;
    if (!onSend(text, running ? [] : images.map(({ image }) => image))) return;
    setText('');
    // A text follow-up cannot carry images. Keep the unsent draft until idle.
    if (!running) setImages([]);
    setProblem(null);
  };

  const pick = async (files: FileList | null): Promise<void> => {
    if (files === null || files.length === 0) return;
    setProblem(null);
    setShrinking(true);
    try {
      const added: DraftImage[] = [];
      for (const file of Array.from(files)) {
        if (images.length + added.length >= MAX_IMAGE_ATTACHMENTS) {
          setProblem(t(locale, 'imageLimit', { count: MAX_IMAGE_ATTACHMENTS }));
          break;
        }
        try {
          const image = await shrinkImage(file);
          added.push({ id: nextImageId.current++, image });
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
      <ComposerSurface>
        {images.length > 0 ? (
          <ul className="composer-image-attachments">
            {images.map(({ id, image }) => (
              <ImageAttachment key={id} image={image} locale={locale}
                onRemove={() => setImages((current) => current.filter((item) => item.id !== id))} />
            ))}
          </ul>
        ) : null}
        {problem ? <p className="rc-note rc-danger" role="alert">{problem}</p> : null}
        {running && images.length === 0 ? <p className="rc-note">{t(locale, 'imagesOnlyWhenIdle')}</p> : null}
        <ComposerEditor><textarea
            className="composer-editor-control"
            aria-label={t(locale, 'composerPlaceholder')}
            value={text}
            rows={1}
            placeholder={t(locale, running ? 'composerPlaceholderRunning' : 'composerPlaceholder')}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && !event.nativeEvent.isComposing && event.keyCode !== 229) {
                event.preventDefault();
                send();
              }
            }}
          /></ComposerEditor>
        <ComposerToolbar>
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
          {running ? (
            <button type="button" className="button secondary" disabled={disabled} onClick={onStop}>
              {t(locale, 'stop')}
            </button>
          ) : null}
          <button type="submit" className="button primary" disabled={!canSend}>
            {t(locale, 'send')}
          </button>
        </ComposerToolbar>
      </ComposerSurface>
    </form>
  );
}
