import type { ConnectionState } from './lingxi';

export type EngineLaunchPhase = 'idle' | 'starting' | 'connecting' | 'ready' | 'error';

export interface EngineLaunchStatus {
  phase: EngineLaunchPhase;
  label: string;
  detail: string;
  percent: number;
  active: boolean;
  canStart: boolean;
}

export function engineLaunchStatus(state: ConnectionState): EngineLaunchStatus {
  switch (state.status) {
    case 'spawning':
    case 'restarting':
      return {
        phase: 'starting',
        label: state.status === 'restarting' ? 'Restarting engine' : 'Starting engine',
        detail: 'Preparing the signed local engine…',
        percent: 32,
        active: true,
        canStart: false,
      };
    case 'connecting':
      return {
        phase: 'connecting',
        label: 'Connecting to engine',
        detail: 'Waiting for the local engine handshake…',
        percent: 72,
        active: true,
        canStart: false,
      };
    case 'connected':
      return {
        phase: 'ready',
        label: 'Engine ready',
        detail: 'The local engine is connected and ready for a session.',
        percent: 100,
        active: false,
        canStart: false,
      };
    case 'error':
      return {
        phase: 'error',
        label: 'Engine failed to start',
        detail: state.message,
        percent: 0,
        active: false,
        canStart: true,
      };
    case 'disconnected':
      return {
        phase: 'error',
        label: 'Engine stopped',
        detail: state.reason ?? 'The local engine is not connected.',
        percent: 0,
        active: false,
        canStart: true,
      };
    case 'idle':
    default:
      return {
        phase: 'idle',
        label: 'Engine not started',
        detail: 'Start the local engine to continue.',
        percent: 0,
        active: false,
        canStart: true,
      };
  }
}
