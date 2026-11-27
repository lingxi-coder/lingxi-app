interface ImageCache<T> {
  get(key: string): T | undefined;
  set(key: string, value: T): unknown;
  clear(): void;
}

interface ImageWork<T> {
  controller: AbortController;
  result: Promise<T>;
  subscribers: number;
}

/** Cache completed images and cancel shared I/O when its last consumer leaves. */
export class CancellableImageRequests<T> {
  private readonly pending = new Map<string, ImageWork<T>>();

  constructor(private readonly cache: ImageCache<T>) {}

  async fetch(key: string, load: (signal: AbortSignal) => Promise<T>, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    const cached = this.cache.get(key);
    if (cached !== undefined) return cached;
    const work = this.pending.get(key) ?? this.start(key, load);
    work.subscribers++;

    return new Promise((resolve, reject) => {
      let done = false;
      const finish = (complete: () => void) => {
        if (done) return;
        done = true;
        work.subscribers--;
        signal?.removeEventListener('abort', cancel);
        complete();
      };
      const cancel = () => {
        if (work.subscribers > 1) {
          finish(() => reject(signal?.reason));
          return;
        }
        if (this.pending.get(key) === work) this.pending.delete(key);
        // Keep the last caller's slot until its underlying I/O acknowledges cancellation.
        work.controller.abort(signal?.reason);
      };
      signal?.addEventListener('abort', cancel, { once: true });
      void work.result.then(
        value => finish(() => signal?.aborted ? reject(signal.reason) : resolve(value)),
        error => finish(() => reject(error)),
      );
      if (signal?.aborted) cancel();
    });
  }

  clear(): void {
    this.cache.clear();
    for (const { controller } of this.pending.values()) controller.abort();
    this.pending.clear();
  }

  private start(key: string, load: (signal: AbortSignal) => Promise<T>): ImageWork<T> {
    const controller = new AbortController();
    const { signal } = controller;
    let begin!: () => void;
    const result = new Promise<T>((resolve, reject) => {
      begin = () => {
        try { resolve(load(signal)); } catch (error) { reject(error); }
      };
    }).then(value => {
      signal.throwIfAborted();
      this.cache.set(key, value);
      return value;
    }).finally(() => {
      if (this.pending.get(key) === work) this.pending.delete(key);
    });
    const work: ImageWork<T> = { controller, result, subscribers: 0 };
    this.pending.set(key, work);
    begin();
    return work;
  }
}
