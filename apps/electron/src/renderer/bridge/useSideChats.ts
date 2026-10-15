import { useCallback, useEffect, useRef, useState } from 'react';
import type { ClientEvent, PermissionRequest, ImageRefDto } from '@lingxi/bridge-client';
import type { AllowedClientCommand } from '../../shared/clientCommands';
import type { ImageAttachment } from './imageInput';
import { acknowledgeSidePrompt, newSideChat, reduceSideChat, sideCommandInput, type SideChatState } from './sideChatState';

type SideEnvelope = { type: string; side_id?: string; side_ids?: string[]; request_id?: string; events_json?: string[]; request?: PermissionRequest; event_json?: string; message?: string };
type Pending = { hostId: string; sideId?: string; text?: string; images?: ImageAttachment[]; resolve(value: string): void; reject(error: Error): void };
export function useSideChats(hostId?: string) {
  const [sides, setSides] = useState<Record<string, SideChatState>>({});
  const [selectedId, setSelectedId] = useState<string>();
  const [open, setOpen] = useState(false);
  const pending = useRef(new Map<string, Pending>());
  const ended = useRef(new Set<string>());
  const sequences = useRef(new Map<string, number>());
  const submitting = useRef(new Set<string>());
  const ending = useRef(new Set<string>());
  const send = useCallback((sessionId: string, command: object) => {
    const host = window.lingxi;
    if (!host) return Promise.reject(new Error('Host is unavailable'));
    return host.command(sessionId, command as AllowedClientCommand);
  }, []);
  const request = useCallback((sessionId: string, command: object, sideId?: string, text?: string, images?: ImageAttachment[]) => {
    const requestId = crypto.randomUUID();
    return new Promise<string>((resolve, reject) => {
      pending.current.set(requestId, { hostId: sessionId, sideId, text, images, resolve, reject });
      void send(sessionId, { ...command, request_id: requestId }).catch((error: Error) => { pending.current.delete(requestId); reject(error); });
    });
  }, [send]);
  const hydrate = useCallback(async (sessionId: string, sideId: string) => {
    // Listings are independent admissions; they must not occupy the prompt latch.
    const commands = [{ type: 'list_models' }, { type: 'get_conversation_controls' }, { type: 'refresh_listings', which: [{ type: 'slash_commands' }] }];
    await Promise.all(commands.map((command) => request(sessionId, { type: 'side_chat_submit', side_id: sideId, command_json: JSON.stringify(command) }, sideId)));
  }, [request]);
  const reportHydration = useCallback((sessionId: string, sideId: string) => {
    void hydrate(sessionId, sideId).catch((error) => setSides((current) => current[sideId] ? { ...current, [sideId]: { ...current[sideId], error: String(error) } } : current));
  }, [hydrate]);
  useEffect(() => {
    const host = window.lingxi;
    if (!host) return;
    return host.onEvent(({ sessionId, sequence, event: raw }) => {
      const event = raw as unknown as SideEnvelope;
      if (event.type === 'session_ended') {
        setSides((current) => Object.fromEntries(Object.entries(current).filter(([, side]) => side.hostId !== sessionId)));
        for (const [id, request] of pending.current) if (request.hostId === sessionId) { request.reject(new Error('Host ended')); pending.current.delete(id); }
        return;
      }
      if (!event.type.startsWith('side_chat_')) return;
      if (Number.isFinite(sequence)) {
        if (sequence <= (sequences.current.get(sessionId) ?? -1)) return;
        sequences.current.set(sessionId, sequence);
      }
      const id = event.side_id;
      if (id && ended.current.has(`${sessionId}:${id}`)) return;
      const request = event.request_id ? pending.current.get(event.request_id) : undefined;
      if (event.type === 'side_chat_created' && id) {
        // Replayed acknowledgements never reopen ended or abandoned creations.
        if (!request || request.hostId !== sessionId) return;
        setSides((current) => current[id] ? current : ({ ...current, [id]: request?.text ? acknowledgeSidePrompt(newSideChat(id, sessionId), request.text) : newSideChat(id, sessionId) }));
        setSelectedId(id); setOpen(true);
        request?.resolve(id);
        reportHydration(sessionId, id);
      } else if (event.type === 'side_chat_list') {
        for (const sideId of event.side_ids ?? []) void send(sessionId, { type: 'side_chat_get', side_id: sideId });
      } else if (event.type === 'side_chat_snapshot' && id) {
        try {
          const events = (event.events_json ?? []).map((json) => JSON.parse(json) as ClientEvent);
          setSides((current) => {
            const previous = current[id];
            let side = newSideChat(id, sessionId);
            for (const inner of events) side = reduceSideChat(side, inner);
            return { ...current, [id]: { ...side, draft: previous?.draft ?? '', images: previous?.images ?? [], closing: previous?.closing ?? false, ending: previous?.ending ?? false, error: previous?.error } };
          });
          reportHydration(sessionId, id);
        } catch { /* A malformed snapshot cannot alter the existing draft. */ }
      } else if (event.type === 'side_chat_event' && id) {
        try {
          const inner = JSON.parse(event.event_json ?? '{}') as ClientEvent;
          setSides((current) => current[id] ? ({ ...current, [id]: reduceSideChat(current[id], inner) }) : current);
        } catch { /* Preserve the last valid RAM state. */ }
      } else if (event.type === 'side_chat_permission' && id && event.request) {
        const permission = event.request;
        setSides((current) => {
          const side = current[id];
          if (!side) return current;
          return { ...current, [id]: { ...side, permissions: [...side.permissions.filter((entry) => entry.request_id !== permission.request_id), permission] } };
        });
      } else if (event.type === 'side_chat_submitted' && id) {
        if (request?.text) setSides((current) => current[id] ? { ...current, [id]: { ...acknowledgeSidePrompt(current[id], request.text!, request.images), images: current[id].images.filter((image) => !request.images?.some((submitted) => submitted.id === image.id)) } } : current);
        request?.resolve(id);
      } else if (event.type === 'side_chat_ended' && id) {
        ending.current.delete(`${sessionId}:${id}`);
        ended.current.add(`${sessionId}:${id}`);
        for (const [requestId, admission] of pending.current) if (admission.sideId === id && admission.hostId === sessionId) {
          admission.reject(new Error('Side Chat ended'));
          pending.current.delete(requestId);
        }
        setSides((current) => { const next = { ...current }; delete next[id]; return next; });
      } else if (event.type === 'side_chat_error') {
        const error = new Error(event.message ?? 'Side Chat request failed');
        // End has no request_id. Keep the child read-only after a failed
        // cleanup, but release its admission latch so End can be retried.
        const endFailed = id && !event.request_id;
        if (endFailed) ending.current.delete(`${sessionId}:${id}`);
        if (id) setSides((current) => current[id] ? { ...current, [id]: { ...current[id], ...(endFailed ? { ending: false } : {}), error: error.message } } : current);
        request?.reject(error);
      }
      if (request && event.request_id && ['side_chat_created', 'side_chat_submitted', 'side_chat_error'].includes(event.type)) pending.current.delete(event.request_id);
    });
  }, [send, reportHydration]);
  useEffect(() => {
    const host = window.lingxi;
    if (!host) return;
    return host.onConnectionStateChanged(({ sessionId, event }) => {
      if (event.status === 'connected') void send(sessionId, { type: 'side_chat_list' }).catch(() => undefined);
    });
  }, [send]);
  useEffect(() => { if (hostId) void send(hostId, { type: 'side_chat_list' }).catch(() => undefined); }, [hostId, send]);
  useEffect(() => () => { for (const request of pending.current.values()) request.reject(new Error('Side Chat closed')); pending.current.clear(); }, []);
  const create = useCallback(async (text?: string) => {
    if (!hostId) throw new Error('Select a session first');
    if (submitting.current.has(hostId)) throw new Error('Side Chat is opening');
    submitting.current.add(hostId);
    try { return await request(hostId, { type: 'side_chat_create', ...(text ? { initial_input: text } : {}) }, undefined, text); }
    finally { submitting.current.delete(hostId); }
  }, [hostId, request]);
  const submit = useCallback(async (side: SideChatState, command: object, text?: string, images?: ImageAttachment[]) => {
    if (side.closing) throw new Error('Side Chat is closing');
    const isPrompt = text !== undefined;
    if (isPrompt && submitting.current.has(side.id)) return;
    if (isPrompt) submitting.current.add(side.id);
    try {
      await request(side.hostId, { type: 'side_chat_submit', side_id: side.id, command_json: JSON.stringify(command) }, side.id, text, images);
      if ('type' in command && command.type === 'set_model') {
        await request(side.hostId, { type: 'side_chat_submit', side_id: side.id, command_json: JSON.stringify({ type: 'get_conversation_controls' }) }, side.id);
      }
    }
    catch (error) { setSides((current) => current[side.id] ? { ...current, [side.id]: { ...current[side.id], error: String(error) } } : current); throw error; }
    finally { if (isPrompt) submitting.current.delete(side.id); }
  }, [request]);
  const visibleSides = Object.values(sides).filter((side) => side.hostId === hostId);
  const selected = visibleSides.find((side) => side.id === selectedId) ?? visibleSides.at(-1);
  return { sides: visibleSides, selected, open, setOpen, select: setSelectedId, create,
    draft(id: string, draft: string) { setSides((current) => current[id] ? ({ ...current, [id]: { ...current[id], draft } }) : current); },
    images(id: string, images: ImageAttachment[]) { setSides((current) => current[id] ? { ...current, [id]: { ...current[id], images } } : current); },
    sendPrompt(side: SideChatState) {
      const command = sideCommandInput(side.draft);
      if (command.type === 'run_slash_command' && side.images.length) return Promise.reject(new Error('Images cannot accompany a slash command.'));
      return submit(side, command.type === 'send_prompt' ? { ...command, images: side.images.map(({ media_type, base64 }): ImageRefDto => ({ media_type, base64 })) } : command, side.draft, side.images);
    },
    submit,
    async end(side: SideChatState) {
      const key = `${side.hostId}:${side.id}`;
      if (ending.current.has(key)) return;
      ending.current.add(key);
      setSides((current) => current[side.id] ? ({ ...current, [side.id]: { ...current[side.id], closing: true, ending: true, error: undefined } }) : current);
      try { await send(side.hostId, { type: 'side_chat_end', side_id: side.id }); }
      catch (error) {
        ending.current.delete(key);
        setSides((current) => current[side.id] ? ({ ...current, [side.id]: { ...current[side.id], ending: false, error: String(error) } }) : current);
      }
    },
  };
}
export type SideChatsController = ReturnType<typeof useSideChats>;
