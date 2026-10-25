import { reasoningSelectionLabel } from '../model/reasoningLabel';
import { useI18n } from '../i18n/useI18n';
import { useRef, useState, type CSSProperties } from 'react';
import { Stage } from './Stage';
import { PermissionPrompt } from './PermissionPrompt';
import { AskUserQuestionPrompt } from './AskUserQuestionPrompt';
import { Icon } from './Icon';
import { useT } from '../theme/ThemeContext';
import { imageFileToAttachment } from '../bridge/imageInput';
import { MAX_IMAGE_ATTACHMENTS } from '../../shared/imageInput';
import { modelReference } from '../bridge/modelCatalog';
import type { SideChatsController } from '../bridge/useSideChats';
import './SideChatPanel.css';

export function SideChatPanel({ controller }: { controller: SideChatsController }) {
  const { tr, locale } = useI18n();
  const t = useT();
  const imageInput = useRef<HTMLInputElement>(null);
  const [width, setWidth] = useState(390);
  const [sendingIds, setSendingIds] = useState<ReadonlySet<string>>(() => new Set());
  const [notice, setNotice] = useState<string>();
  const side = controller.selected;
  if (!controller.open || !side) return null;
  const controls = side.desktop.conversationControls;
  const sending = sendingIds.has(side.id);
  const act = (operation: Promise<unknown>) => { void operation.catch((error: Error) => setNotice(error.message)); };
  const attach = async (files: File[]) => {
    if (side.images.length + files.length > MAX_IMAGE_ATTACHMENTS) { setNotice(tr('desktop_chat_image_limit', MAX_IMAGE_ATTACHMENTS)); return; }
    try { const images = await Promise.all(files.map(imageFileToAttachment)); controller.images(side.id, [...side.images, ...images]); }
    catch (error) { setNotice(String(error)); }
  };
  const send = async () => {
    if (!side.draft.trim() || sending || side.closing || side.conversation.running) return;
    setSendingIds((current) => new Set(current).add(side.id)); setNotice(undefined);
    try { await controller.sendPrompt(side); } catch (error) { setNotice(String(error)); }
    finally { setSendingIds((current) => { const next = new Set(current); next.delete(side.id); return next; }); }
  };
  return <aside className="side-chat-panel no-drag" aria-label={tr('desktop_navigation_side_chat')} style={{ width, '--side-bg': t.stageBg, '--side-text': t.text, '--side-muted': t.text3, '--side-border': t.border, '--side-hover': t.surfaceHover, '--side-surface': t.surface } as CSSProperties}>
    <div className="side-chat-resize" role="separator" aria-label={tr('desktop_navigation_resize_side')} aria-orientation="vertical" tabIndex={0} aria-valuemin={320} aria-valuemax={800} aria-valuenow={width}
      onKeyDown={(event) => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setWidth((current) => Math.max(320, Math.min(800, current + (event.key === 'ArrowLeft' ? 16 : -16)))); } }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        const startX = event.clientX; const startWidth = width; const target = event.currentTarget;
        const move = (next: PointerEvent) => setWidth(Math.max(320, Math.min(800, startWidth + startX - next.clientX)));
        const end = () => { target.removeEventListener('pointermove', move); target.removeEventListener('pointerup', end); target.removeEventListener('pointercancel', end); };
        target.addEventListener('pointermove', move); target.addEventListener('pointerup', end); target.addEventListener('pointercancel', end);
      }} />
    <header className="side-chat-header">
      <select aria-label={tr('desktop_navigation_select_side')} value={side.id} onChange={(event) => { controller.select(event.target.value); setNotice(undefined); }}>
        {controller.sides.map((entry, index) => <option key={entry.id} value={entry.id}>{tr('desktop_navigation_side_chat')} {index + 1}</option>)}
      </select>
      <button title={tr('desktop_navigation_new_side')} aria-label={tr('desktop_navigation_new_side')} onClick={() => act(controller.create())}><Icon name="plus" size={16} /></button>
      <button title={side.closing ? tr('desktop_navigation_retry_end_side') : tr('desktop_navigation_end_side')} aria-label={tr('desktop_navigation_end_side')} disabled={side.ending} onClick={() => act(controller.end(side))}><span className="side-chat-aux-label">{side.closing && !side.ending ? tr('desktop_navigation_retry_end') : tr('desktop_navigation_end')}</span><span className="side-chat-aux-icon">×</span></button>
      <button title={tr('desktop_navigation_hide_side')} aria-label={tr('desktop_navigation_hide_side')} onClick={() => controller.setOpen(false)}><Icon name="topbar-inspector-open" size={18} /></button>
    </header>
    <div className="side-chat-body">
      <Stage liveItems={side.conversation.items} running={side.conversation.running} apiRetry={side.desktop.apiRetry} sessionKey={`side:${side.id}`} emptyMessage={tr('desktop_navigation_side_empty')} />
      <div className="side-chat-interactions">
        <PermissionPrompt allowPersistentRules={false} request={side.permissions[0] ?? null} onApprove={(request_id, response) => act(controller.submit(side, { type: 'approve_permission', request_id, response }))} onDeny={(request_id) => act(controller.submit(side, { type: 'deny_permission', request_id }))} />
        {side.questions[0] ? <AskUserQuestionPrompt request={side.questions[0]} onSubmit={(request_id, answers) => act(controller.submit(side, { type: 'answer_ask_user_question', request_id, answers }))} onCancel={(request_id) => act(controller.submit(side, { type: 'cancel_ask_user_question', request_id }))} /> : null}
      </div>
    </div>
    <div className="side-chat-composer-area">
      {(notice || side.error || side.closing) ? <div role="status" className="side-chat-notice">{notice ?? side.error ?? tr('desktop_navigation_closing_side')}</div> : null}
      <div className="side-chat-composer">
        <input ref={imageInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden onChange={(event) => { void attach(Array.from(event.target.files ?? [])); event.target.value = ''; }} />
        {side.images.length ? <div className="side-chat-images">{side.images.map((image) => <button key={image.id} title={tr('desktop_chat_remove_named', image.name)} aria-label={tr('desktop_chat_remove_named', image.name)} onClick={() => controller.images(side.id, side.images.filter((entry) => entry.id !== image.id))}><img src={image.previewUrl} alt={image.name} /><span>×</span></button>)}</div> : null}
        <textarea aria-label={tr('desktop_navigation_side_message')} onPaste={(event) => { const files = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith('image/')); if (files.length) { event.preventDefault(); void attach(files); } }} placeholder={tr('desktop_navigation_ask_lingxi')} value={side.draft} disabled={side.closing} onChange={(event) => controller.draft(side.id, event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send(); } }} />
        <div className="side-chat-controls">
          <button title={tr('desktop_navigation_attach_image')} aria-label={tr('desktop_navigation_attach_side_image')} disabled={side.closing} onClick={() => imageInput.current?.click()}><Icon name="plus" size={17} /></button>
          <span className="side-chat-aux-label side-chat-context" title={tr('desktop_navigation_parent_snapshot_description')}>{tr('desktop_navigation_parent_context')}</span>
          <span className="side-chat-aux-icon side-chat-context" title={tr('desktop_navigation_parent_snapshot')} aria-label={tr('desktop_navigation_parent_snapshot')}>◷</span>
          <select aria-label={tr('desktop_navigation_side_model')} title={tr('desktop_navigation_side_model_description')} value={controls?.qualified_model ?? side.desktop.currentModel ?? ''} disabled={side.closing || !side.desktop.models.length} onChange={(event) => act(controller.submit(side, { type: 'set_model', model: event.target.value }))}>
            {!side.desktop.models.length ? <option value="">{tr('desktop_navigation_inherited_model')}</option> : null}
            {side.desktop.models.map((model) => <option key={model} value={model}>{modelReference(model).label}</option>)}
          </select>
          <select aria-label={tr('desktop_navigation_side_reasoning')} title={tr('desktop_navigation_side_reasoning_description')} value={JSON.stringify(controls?.reasoning.requested ?? {})} disabled={side.closing || !controls?.reasoning.spec.editable} onChange={(event) => act(controller.submit(side, { type: 'set_reasoning_selection', selection: JSON.parse(event.target.value) }))}>
            {!controls ? <option value="{}">{tr('composer_effort')}</option> : controls.reasoning.spec.options.map((option) => <option key={JSON.stringify(option.selection)} value={JSON.stringify(option.selection)}>{reasoningSelectionLabel(option.selection, locale)}</option>)}
          </select>
          {side.conversation.running ? <button className="side-chat-send" aria-label={tr('desktop_navigation_stop_side')} title={tr('desktop_navigation_stop_side')} disabled={side.closing} onClick={() => act(controller.submit(side, { type: 'cancel' }))}>■</button> : <button className="side-chat-send" aria-label={tr('desktop_navigation_send_side')} title={tr('composer_send')} disabled={sending || side.closing || !side.draft.trim()} onClick={() => void send()}>↑</button>}
        </div>
      </div>
    </div>
  </aside>;
}
