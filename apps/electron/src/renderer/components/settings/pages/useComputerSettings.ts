import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopHost } from '../../../bridge/bridgeConnection';
import type { ComputerSettingsCommand, ComputerSettingsSnapshot } from '../../../../shared/computerSettings';

export type ComputerSettingsMutation = Exclude<ComputerSettingsCommand, { action: 'get' }>;
export interface ComputerSettingsController {
  state: ComputerSettingsSnapshot | null;
  pending: boolean;
  error: string | null;
  refresh(): Promise<void>;
  execute(command: ComputerSettingsMutation): Promise<boolean>;
}

/** Only host-confirmed snapshots become visible; failed writes never look saved. */
export function useComputerSettings(): ComputerSettingsController {
  const [state, setState] = useState<ComputerSettingsSnapshot | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(false);
  const busy = useRef(false);
  const events = useRef(0);
  const reads = useRef(0);

  const refresh = useCallback(async () => {
    const api = desktopHost()?.computerSettings;
    const read = ++reads.current;
    const observedEvents = events.current;
    if (!api) {
      if (alive.current) setError('Computer access settings are unavailable in this window.');
      return;
    }
    if (alive.current) setError(null);
    try {
      const next = await api.execute({ action: 'get' });
      if (alive.current && read === reads.current && observedEvents === events.current) setState(next);
    } catch (cause) {
      if (alive.current && read === reads.current) setError(cause instanceof Error ? cause.message : 'Could not load computer access settings.');
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    const off = desktopHost()?.computerSettings?.onChanged((next) => {
      events.current += 1;
      if (alive.current) setState(next);
    });
    void refresh();
    return () => { alive.current = false; off?.(); };
  }, [refresh]);

  const execute = useCallback(async (command: ComputerSettingsMutation) => {
    const api = desktopHost()?.computerSettings;
    if (!api || busy.current) return false;
    busy.current = true;
    setPending(true);
    setError(null);
    const observedEvents = events.current;
    try {
      const next = await api.execute(command);
      if (alive.current && observedEvents === events.current) setState(next);
      // A push may describe another window's newer policy. Re-read instead of
      // replacing it with the result of an earlier mutation.
      if (observedEvents !== events.current) await refresh();
      return true;
    } catch (cause) {
      await refresh();
      if (alive.current) setError(cause instanceof Error ? cause.message : 'Could not update computer access settings.');
      return false;
    } finally {
      busy.current = false;
      if (alive.current) setPending(false);
    }
  }, [refresh]);

  return { state, pending, error: state?.error ?? error, refresh, execute };
}
