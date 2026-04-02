import type { ActorDescriptor } from "./descriptor.js";
import { CachedStream } from "./stream.js";

/**
 * Module-private interface for the WASM-generated handle.
 * Each actor's generated code produces a class that satisfies this shape.
 */
interface WasmActorHandle {
  readonly id: string;
  tell(msg: unknown): void;
  ask(msg: unknown): Promise<unknown>;
  prep(): unknown;
  initStream(callback: (view: unknown) => void): void;
  free?(): void;
}

/**
 * Typed actor reference for JS/TS callers.
 *
 * Usage:
 * ```ts
 * const ref = ActorRef.from<ChatRoom>(handle);
 * ref.tell({ SendMessage: { text: "hello" } });
 * const state = await ref.subscribe();
 * ```
 */
export class ActorRef<A extends ActorDescriptor> {
  /** Global cache: actorId → Promise<CachedStream<A["View"]>> */
  private static _streams = new Map<string, Promise<CachedStream<unknown>>>();

  private readonly _handle: WasmActorHandle;

  private constructor(handle: WasmActorHandle) {
    this._handle = handle;
  }

  /** Wrap a WASM handle into a typed ActorRef. */
  static from<A extends ActorDescriptor>(handle: unknown): ActorRef<A> {
    return new ActorRef<A>(handle as WasmActorHandle);
  }

  /** The actor's unique identifier. */
  get id(): string {
    return this._handle.id;
  }

  /** Fire-and-forget message send. */
  tell(msg: A["Msg"]): void {
    this._handle.tell(msg);
  }

  /**
   * Send a message and wait for a response.
   * The key of the message object determines which return type is produced.
   */
  async ask<K extends keyof A["Returns"]>(
    msg: { [P in K]: unknown } & A["Msg"]
  ): Promise<A["Returns"][K]> {
    const result = await this._handle.ask(msg);
    return result as A["Returns"][K];
  }

  /**
   * Get or create a globally-cached stream of this actor's state (View).
   *
   * Multiple callers for the same actor ID share a single stream.
   * The returned CachedStream holds the latest View value.
   */
  subscribe(): Promise<CachedStream<A["View"]>> {
    const id = this._handle.id;
    const existing = ActorRef._streams.get(id);
    if (existing) {
      return existing as Promise<CachedStream<A["View"]>>;
    }

    const promise = new Promise<CachedStream<A["View"]>>((resolve) => {
      const stream = new CachedStream<A["View"]>();

      // Get initial state via prep
      const initial = this._handle.prep() as A["View"] | null;
      if (initial != null) {
        stream._push(initial);
      }

      // Start streaming updates
      this._handle.initStream((view: unknown) => {
        stream._push(view as A["View"]);
      });

      resolve(stream);
    });

    ActorRef._streams.set(id, promise as Promise<CachedStream<unknown>>);
    return promise;
  }
}
