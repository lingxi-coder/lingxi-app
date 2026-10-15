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
    if (side.images.length + files.length > MAX_IMAGE_ATTACHMENTS) { setNotice(`Attach up to ${MAX_IMAGE_ATTACHMENTS} images.`); return; }
    try { const images = await Promise.all(files.map(imageFileToAttachment)); controller.images(side.id, [...side.images, ...images]); }
    catch (error) { setNotice(String(error)); }
  };
  const send = async () => {
    if (!side.draft.trim() || sending || side.closing || side.conversation.running) return;
    setSendingIds((current) => new Set(current).add(side.id)); setNotice(undefined);
    try { await controller.sendPrompt(side); } catch (error) { setNotice(String(error)); }
    finally { setSendingIds((current) => { const next = new Set(current); next.delete(side.id); return next; }); }
  };
  return <aside className="side-chat-panel no-drag" aria-label="Side Chat" style={{ width, '--side-bg': t.stageBg, '--side-text': t.text, '--side-muted': t.text3, '--side-border': t.border, '--side-hover': t.surfaceHover, '--side-surface': t.surface } as CSSProperties}>
    <div className="side-chat-resize" role="separator" aria-label="Resize Side Chat" aria-orientation="vertical" tabIndex={0} aria-valuemin={320} aria-valuemax={800} aria-valuenow={width}
      onKeyDown={(event) => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setWidth((current) => Math.max(320, Math.min(800, current + (event.key === 'ArrowLeft' ? 16 : -16)))); } }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        const startX = event.clientX; const startWidth = width; const target = event.currentTarget;
        const move = (next: PointerEvent) => setWidth(Math.max(320, Math.min(800, startWidth + startX - next.clientX)));
        const end = () => { target.removeEventListener('pointermove', move); target.removeEventListener('pointerup', end); target.removeEventListener('pointercancel', end); };
        target.addEventListener('pointermove', move); target.addEventListener('pointerup', end); target.addEventListener('pointercancel', end);
      }} />
    <header className="side-chat-header">
      <select aria-label="Select Side Chat" value={side.id} onChange={(event) => { controller.select(event.target.value); setNotice(undefined); }}>
        {controller.sides.map((entry, index) => <option key={entry.id} value={entry.id}>Side Chat {index + 1}</option>)}
      </select>
      <button title="New Side Chat" aria-label="New Side Chat" onClick={() => act(controller.create())}><Icon name="plus" size={16} /></button>
      <button title={side.closing ? 'Retry ending Side Chat' : 'End Side Chat'} aria-label="End Side Chat" disabled={side.ending} onClick={() => act(controller.end(side))}><span className="side-chat-aux-label">{side.closing && !side.ending ? 'Retry End' : 'End'}</span><span className="side-chat-aux-icon">×</span></button>
      <button title="Hide Side Chat" aria-label="Hide Side Chat" onClick={() => controller.setOpen(false)}><Icon name="topbar-inspector-open" size={18} /></button>
    </header>
    <div className="side-chat-body">
      <Stage liveItems={side.conversation.items} running={side.conversation.running} apiRetry={side.desktop.apiRetry} sessionKey={`side:${side.id}`} emptyMessage="Ask a question using this chat’s context." />
      <div className="side-chat-interactions">
        <PermissionPrompt allowPersistentRules={false} request={side.permissions[0] ?? null} onApprove={(request_id, response) => act(controller.submit(side, { type: 'approve_permission', request_id, response }))} onDeny={(request_id) => act(controller.submit(side, { type: 'deny_permission', request_id }))} />
        {side.questions[0] ? <AskUserQuestionPrompt request={side.questions[0]} onSubmit={(request_id, answers) => act(controller.submit(side, { type: 'answer_ask_user_question', request_id, answers }))} onCancel={(request_id) => act(controller.submit(side, { type: 'cancel_ask_user_question', request_id }))} /> : null}
      </div>
    </div>
    <div className="side-chat-composer-area">
      {(notice || side.error || side.closing) ? <div role="status" className="side-chat-notice">{notice ?? side.error ?? 'Closing Side Chat…'}</div> : null}
      <div className="side-chat-composer">
        <input ref={imageInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden onChange={(event) => { void attach(Array.from(event.target.files ?? [])); event.target.value = ''; }} />
        {side.images.length ? <div className="side-chat-images">{side.images.map((image) => <button key={image.id} title={`Remove ${image.name}`} aria-label={`Remove ${image.name}`} onClick={() => controller.images(side.id, side.images.filter((entry) => entry.id !== image.id))}><img src={image.previewUrl} alt={image.name} /><span>×</span></button>)}</div> : null}
        <textarea aria-label="Side Chat message" onPaste={(event) => { const files = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith('image/')); if (files.length) { event.preventDefault(); void attach(files); } }} placeholder="Ask LingXi" value={side.draft} disabled={side.closing} onChange={(event) => controller.draft(side.id, event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send(); } }} />
        <div className="side-chat-controls">
          <button title="Attach image" aria-label="Attach image to Side Chat" disabled={side.closing} onClick={() => imageInput.current?.click()}><Icon name="plus" size={17} /></button>
          <span className="side-chat-aux-label side-chat-context" title="This chat uses a snapshot of the parent context">Parent context</span>
          <span className="side-chat-aux-icon side-chat-context" title="Parent context snapshot" aria-label="Parent context snapshot">◷</span>
          <select aria-label="Side Chat model" title="Model for this Side Chat" value={controls?.qualified_model ?? side.desktop.currentModel ?? ''} disabled={side.closing || !side.desktop.models.length} onChange={(event) => act(controller.submit(side, { type: 'set_model', model: event.target.value }))}>
            {!side.desktop.models.length ? <option value="">Inherited model</option> : null}
            {side.desktop.models.map((model) => <option key={model} value={model}>{modelReference(model).label}</option>)}
          </select>
          <select aria-label="Side Chat reasoning effort" title="Reasoning effort for this Side Chat" value={JSON.stringify(controls?.reasoning.requested ?? {})} disabled={side.closing || !controls?.reasoning.spec.editable} onChange={(event) => act(controller.submit(side, { type: 'set_reasoning_selection', selection: JSON.parse(event.target.value) }))}>
            {!controls ? <option value="{}">Effort</option> : controls.reasoning.spec.options.map((option) => <option key={JSON.stringify(option.selection)} value={JSON.stringify(option.selection)}>{option.selection.type === 'level' ? option.selection.id : option.selection.type === 'token_budget' ? `${option.selection.tokens} tokens` : option.selection.type}</option>)}
          </select>
          {side.conversation.running ? <button className="side-chat-send" aria-label="Stop Side Chat" title="Stop Side Chat" disabled={side.closing} onClick={() => act(controller.submit(side, { type: 'cancel' }))}>■</button> : <button className="side-chat-send" aria-label="Send Side Chat message" title="Send" disabled={sending || side.closing || !side.draft.trim()} onClick={() => void send()}>↑</button>}
        </div>
      </div>
    </div>
  </aside>;
}
