import { appendFile, mkdir, rename, stat, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const FILE_BYTES = 2_000_000;
const QUEUE_ENTRIES = 1_024;
const QUEUE_BYTES = 1024 * 1024;
const BATCH_ENTRIES = 64;
const BATCH_BYTES = 64 * 1024;

interface PendingLine {
  text: string;
  bytes: number;
}

/** One bounded queue and serial writer per file; failures have their own queue. */
export class DiagnosticFileWriter {
  private readonly pending: Array<PendingLine | undefined> = new Array(QUEUE_ENTRIES);
  private head = 0;
  private count = 0;
  private bytes = 0;
  private dropped = 0;
  private scheduled: NodeJS.Immediate | undefined;
  private running: Promise<void> | undefined;
  private directoryReady = false;
  private readonly tail: string[];

  constructor(
    private readonly filePath: string,
    private readonly kind: 'live' | 'errors',
    private readonly retainedEntries = 0,
    initialTail: readonly string[] = [],
  ) {
    this.tail = retainedEntries ? initialTail.slice(-retainedEntries) : [];
  }

  add(text: string): void {
    const bytes = Buffer.byteLength(text);
    if (bytes > QUEUE_BYTES) {
      this.dropped++;
    } else {
      while (this.count && (this.count === QUEUE_ENTRIES || this.bytes + bytes > QUEUE_BYTES)) {
        this.take();
        this.dropped++;
      }
      this.pending[(this.head + this.count) % QUEUE_ENTRIES] = { text, bytes };
      this.count++;
      this.bytes += bytes;
    }
    this.schedule();
  }

  async flush(): Promise<void> {
    while (this.running || this.count || this.dropped) {
      if (this.scheduled) {
        clearImmediate(this.scheduled);
        this.scheduled = undefined;
      }
      await this.start();
    }
  }

  private take(): PendingLine {
    const line = this.pending[this.head]!;
    this.pending[this.head] = undefined;
    this.head = (this.head + 1) % QUEUE_ENTRIES;
    this.count--;
    this.bytes -= line.bytes;
    return line;
  }

  private schedule(): void {
    if (this.running || this.scheduled) return;
    this.scheduled = setImmediate(() => {
      this.scheduled = undefined;
      void this.start();
    });
  }

  private start(): Promise<void> {
    if (!this.running) {
      this.running = this.drain().finally(() => {
        this.running = undefined;
        if (this.count || this.dropped) this.schedule();
      });
    }
    return this.running;
  }

  private async drain(): Promise<void> {
    while (this.count || this.dropped) {
      const lines: string[] = [];
      let bytes = 0;
      if (this.dropped) {
        const marker = `${JSON.stringify({
          timestamp: new Date().toISOString(), level: 'warn', source: 'host',
          message: JSON.stringify({ event: 'diagnostic_backlog_dropped', queue: this.kind, count: this.dropped }),
        })}\n`;
        lines.push(marker);
        bytes += Buffer.byteLength(marker);
        this.dropped = 0;
      }
      while (this.count && lines.length < BATCH_ENTRIES) {
        const next = this.pending[this.head]!;
        if (lines.length && bytes + next.bytes > BATCH_BYTES) break;
        lines.push(this.take().text);
        bytes += next.bytes;
      }
      // Only dequeued lines enter the rotation tail. A snapshot of the public
      // buffer could include future batches, which would then be appended twice.
      if (this.retainedEntries) {
        this.tail.push(...lines);
        if (this.tail.length > this.retainedEntries) this.tail.splice(0, this.tail.length - this.retainedEntries);
      }
      const retainedTail = this.retainedEntries ? this.tail.join('') : undefined;
      try {
        await this.persist(lines.join(''), bytes, retainedTail);
      } catch {
        // Missing/unwritable/full storage must not stall the desktop or build
        // an unbounded retry queue. The next batch may retry directory setup.
        this.directoryReady = false;
      }
    }
  }

  private async persist(body: string, bytes: number, retainedTail: string | undefined): Promise<void> {
    if (!this.directoryReady) {
      await mkdir(dirname(this.filePath), { recursive: true, mode: 0o700 });
      this.directoryReady = true;
    }
    const size = await stat(this.filePath).then(value => value.size, (error: NodeJS.ErrnoException) => {
      if (error.code === 'ENOENT') return 0;
      throw error;
    });
    if (size && size + bytes >= FILE_BYTES) {
      await rename(this.filePath, `${this.filePath}.1`);
      if (retainedTail !== undefined) {
        const temporary = `${this.filePath}.tmp`;
        await writeFile(temporary, retainedTail, { mode: 0o600 });
        await rename(temporary, this.filePath);
        return;
      }
    }
    await appendFile(this.filePath, body, { mode: 0o600 });
  }
}
