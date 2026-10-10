export const CH_REALTIME_AUDIO_COMMAND = 'lingxi:audio:realtime-command';
export const CH_REALTIME_AUDIO_STATE = 'lingxi:audio:realtime-state';
export interface NativeRealtimeAudioState {
  phase: 'requestingPermission' | 'configurationRequired' | 'listening' | 'recognizing' | 'thinking' | 'speaking' | 'interrupting' | 'paused' | 'failed';
  detail: string;
  generation: number;
  sessionId?: string;
}
export type NativeRealtimeAudioCommand = 'start' | 'commit' | 'stop';
