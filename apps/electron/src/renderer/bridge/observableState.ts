/**
 * State that lives outside React and is read with `useSyncExternalStore`. The
 * Bots page keeps its lists and its half-finished edits in controllers built on
 * this, so that what they do (load, edit, save, confirm a deletion) can be
 * exercised without a rendered tree.
 */
export class ObservableState<S> {
  private current: S;
  private readonly listeners = new Set<() => void>();

  constructor(initial: S) {
    this.current = initial;
  }

  /** Arrow properties: `useSyncExternalStore` calls these detached from the object. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  readonly getSnapshot = (): S => this.current;

  protected set(next: S): void {
    this.current = next;
    for (const listener of [...this.listeners]) listener();
  }

  protected update(change: Partial<S>): void {
    this.set({ ...this.current, ...change });
  }
}
