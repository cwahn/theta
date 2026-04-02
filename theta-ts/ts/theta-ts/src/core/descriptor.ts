/**
 * Descriptor interface that every actor's generated TS types must satisfy.
 * Used as the single type parameter for `ActorRef<A>`.
 */
export interface ActorDescriptor {
  readonly Msg: unknown;
  readonly Returns: Record<string, unknown>;
  readonly View: unknown;
}
