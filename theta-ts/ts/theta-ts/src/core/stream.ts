/**
 * A cached stream that holds the latest value and notifies subscribers.
 * Mirrors the stream caching pattern from theta-frb.
 */
export class CachedStream<T> {
  private _value: T | null = null;
  private readonly _listeners = new Set<() => void>();

  get value(): T | null {
    return this._value;
  }

  /** Push a new value and notify all subscribers. */
  _push(value: T): void {
    this._value = value;
    for (const listener of this._listeners) {
      listener();
    }
  }

  /**
   * Subscribe to value changes.
   * @returns An unsubscribe function.
   */
  subscribe(listener: () => void): () => void {
    this._listeners.add(listener);
    return () => {
      this._listeners.delete(listener);
    };
  }
}
