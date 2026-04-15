/**
 * An object that supports state streaming — matches the shape of generated XRef classes.
 */
export interface ActorStreamable<V> {
  readonly id: string;
  prep(): Promise<V>;
  initStream(callback: (state: V) => void): Promise<void>;
}

/**
 * Holds the latest value from an actor's state stream with a
 * `useSyncExternalStore`-compatible subscribe/getSnapshot contract.
 */
export class CachedStream<V> {
  private _value: V | null = null;
  private _listeners = new Set<() => void>();

  get value(): V | null {
    return this._value;
  }

  /** @internal */
  _push(v: V): void {
    this._value = v;
    for (const l of this._listeners) l();
  }

  subscribe(listener: () => void): () => void {
    this._listeners.add(listener);
    return () => {
      this._listeners.delete(listener);
    };
  }
}

/** One stream per actor ID — prevents duplicate monitors. */
const streams = new Map<string, CachedStream<any>>();

/**
 * Get or create a `CachedStream` for the given actor ref.
 * The first call starts the underlying `initStream` monitor;
 * subsequent calls for the same actor ID return the existing stream.
 */
export function createStream<V>(ref: ActorStreamable<V>): CachedStream<V> {
  const existing = streams.get(ref.id);
  if (existing) return existing;

  const stream = new CachedStream<V>();
  streams.set(ref.id, stream);

  ref.initStream((state) => {
    stream._push(state);
  }).catch((e) => {
    console.error(`[theta] createStream failed for ${ref.id}:`, e);
  });

  return stream;
}
