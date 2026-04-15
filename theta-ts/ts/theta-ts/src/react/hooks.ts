import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { createStream, type ActorStreamable } from "../core/stream.js";

/**
 * Subscribe to an actor's view with an optional selector.
 * Returns the (selected) state, or `null` while the first snapshot is in flight.
 *
 * Uses `useSyncExternalStore` for concurrent-mode safety.
 */
export function useActorView<V, S = V>(
  ref: ActorStreamable<V> | null,
  selector?: (state: V) => S,
  equalityFn?: (a: S, b: S) => boolean,
): S | null {
  const stream = ref ? createStream(ref) : null;

  // Keep selector/equality in refs so subscribe & getSnapshot stay stable.
  const selectorRef = useRef(selector);
  const equalityRef = useRef(equalityFn);
  const prevValueRef = useRef<V | null>(null);
  const prevSliceRef = useRef<S | null>(null);
  selectorRef.current = selector;
  equalityRef.current = equalityFn;

  const getSnapshot = useCallback((): S | null => {
    if (!stream) return null;
    const value = stream.value;
    if (value === null) return null;

    const sel = selectorRef.current;
    if (!sel) return value as unknown as S;

    // Same raw value → reuse cached slice (avoids re-running selector).
    if (Object.is(value, prevValueRef.current) && prevSliceRef.current !== null) {
      return prevSliceRef.current;
    }

    const nextSlice = sel(value);
    const eq = equalityRef.current;

    // Custom equality: return prev reference so useSyncExternalStore skips re-render.
    if (eq && prevSliceRef.current !== null && eq(prevSliceRef.current, nextSlice)) {
      prevValueRef.current = value;
      return prevSliceRef.current;
    }

    prevValueRef.current = value;
    prevSliceRef.current = nextSlice;
    return nextSlice;
  }, [stream]);

  const subscribe = useCallback(
    (onStoreChange: () => void) => {
      if (!stream) return () => {};
      return stream.subscribe(onStoreChange);
    },
    [stream],
  );

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * Look up an ActorRef (sync or async). Re-runs when `deps` change.
 */
export function useActorRef<R>(
  lookupFn: () => R | Promise<R>,
  deps: React.DependencyList = [],
): R | null {
  const [ref, setRef] = useState<R | null>(null);

  useEffect(() => {
    let cancelled = false;
    const result = lookupFn();
    if (result instanceof Promise) {
      result
        .then((r) => { if (!cancelled) setRef(r); })
        .catch(() => { if (!cancelled) setRef(null); });
    } else {
      setRef(result);
    }
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return ref;
}
