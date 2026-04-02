import { useSyncExternalStore, useState, useEffect } from "react";
import type { ActorDescriptor } from "../core/descriptor.js";
import type { ActorRef } from "../core/ref.js";
import type { CachedStream } from "../core/stream.js";

/**
 * Look up an ActorRef. Re-runs the lookup when `deps` change.
 * Lookup and subscription are intentionally separated.
 *
 * @param lookupFn - A function that returns an `ActorRef<A>` (e.g. wrapping a WASM lookup call).
 * @param deps - Dependency array that triggers re-lookup when changed.
 */
export function useActorRef<A extends ActorDescriptor>(
  lookupFn: () => ActorRef<A>,
  deps: React.DependencyList = []
): ActorRef<A> | null {
  const [ref, setRef] = useState<ActorRef<A> | null>(null);

  useEffect(() => {
    try {
      setRef(lookupFn());
    } catch {
      setRef(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return ref;
}

/**
 * Subscribe to an actor's View (state stream).
 * Returns the latest View snapshot, or `null` if not yet available.
 *
 * Uses `useSyncExternalStore` for tear-free reads compatible with React 18 concurrent mode.
 *
 * @param actorRef - The actor reference to subscribe to (may be null).
 */
export function useActorState<A extends ActorDescriptor>(
  actorRef: ActorRef<A> | null
): A["View"] | null {
  const [stream, setStream] = useState<CachedStream<A["View"]> | null>(null);

  useEffect(() => {
    if (!actorRef) {
      setStream(null);
      return;
    }

    let cancelled = false;
    actorRef.subscribe().then((s) => {
      if (!cancelled) setStream(s);
    });

    return () => {
      cancelled = true;
    };
  }, [actorRef]);

  const subscribe = (onStoreChange: () => void) => {
    if (!stream) return () => {};
    return stream.subscribe(onStoreChange);
  };

  const getSnapshot = () => {
    return stream?.value ?? null;
  };

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
