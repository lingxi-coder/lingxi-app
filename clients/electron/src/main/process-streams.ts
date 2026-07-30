interface ErrorEmitter {
  on(event: 'error', listener: (error: Error) => void): unknown;
}

/**
 * Electron reports rejected IPC calls through the process console. A detached
 * development launcher can close that pipe before Electron writes the report;
 * the resulting EPIPE is transport cleanup, not an application failure.
 */
export function ignoreBrokenPipe(stream: ErrorEmitter): void {
  stream.on('error', (error) => {
    if ((error as NodeJS.ErrnoException).code !== 'EPIPE') throw error;
  });
}
