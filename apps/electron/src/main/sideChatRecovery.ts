import type { ClientCommand, ClientEvent } from '@lingxi/bridge-client';

interface Checkpoint {
  context: string;
  events: string[];
}
interface PendingRestore {
  resolve(): void;
  reject(error: Error): void;
}

/** Private model context stays in the host and never enters renderer/remote replay. */
export class SideChatRecovery {
  private readonly checkpoints = new Map<string, Checkpoint>();
  private readonly pending = new Map<string, PendingRestore>();

  get size(): number {
    return this.checkpoints.size;
  }

  observe(event: ClientEvent): boolean {
    if (event.type === 'side_chat_checkpoint') {
      this.checkpoints.set(event.side_id, {
        context: event.checkpoint_json,
        events: event.events_json,
      });
      return true;
    }
    if (event.type === 'side_chat_event')
      this.checkpoints.get(event.side_id)?.events.push(event.event_json);
    if (event.type === 'side_chat_permission')
      this.checkpoints.get(event.side_id)?.events.push(JSON.stringify(event));
    if (event.type === 'side_chat_snapshot') {
      const checkpoint = this.checkpoints.get(event.side_id);
      if (checkpoint) checkpoint.events = event.events_json;
      this.pending.get(event.side_id)?.resolve();
    }
    if (event.type === 'side_chat_error' && event.side_id)
      this.pending.get(event.side_id)?.reject(new Error(event.message));
    if (event.type === 'side_chat_ended') {
      this.checkpoints.delete(event.side_id);
      this.pending
        .get(event.side_id)
        ?.reject(new Error('Side Chat ended during recovery.'));
    }
    return false;
  }

  async restore(send: (command: ClientCommand) => void): Promise<void> {
    for (const [id, checkpoint] of [...this.checkpoints]) {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(
          () => finish(new Error('Side Chat recovery timed out.')),
          15_000,
        );
        timer.unref();
        const finish = (error?: Error): void => {
          clearTimeout(timer);
          this.pending.delete(id);
          if (error) reject(error);
          else resolve();
        };
        this.pending.set(id, { resolve: () => finish(), reject: finish });
        try {
          send({
            type: 'side_chat_restore',
            side_id: id,
            checkpoint_json: checkpoint.context,
            events_json: checkpoint.events,
          });
        } catch (error) {
          finish(error instanceof Error ? error : new Error(String(error)));
        }
      });
    }
  }

  interrupt(): void {
    for (const pending of this.pending.values())
      pending.reject(new Error('Side Chat recovery was interrupted.'));
    this.pending.clear();
  }

  clear(): void {
    this.interrupt();
    this.checkpoints.clear();
  }
}
