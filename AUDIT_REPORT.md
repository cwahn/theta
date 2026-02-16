# Theta Audit Report

Branch: `fix/audit-bugs-tdd`  
Base: `main` @ `54932e3`

---

## P0 — Confirmed Bugs (Fixed)

### P0#1: `terminate()` passes `ExitCode::Dropped` instead of `ExitCode::Terminated`

**File:** `theta/src/actor_instance.rs` line 435  
**Real bug?** Yes — confirmed by failing test.

**Problem:**  
When an actor is explicitly terminated (via `Terminate` signal), the `terminate()` method calls `self.on_exit(ExitCode::Dropped)`. This means `on_exit` receives `Dropped`, making it impossible to distinguish between an actor whose references were all dropped vs. one that was intentionally terminated.

**Fix:**  
One-line change: `ExitCode::Dropped` → `ExitCode::Terminated` in the `terminate()` method.

**Test:** `p0_1_terminate_should_pass_exit_code_terminated` — spawns an actor, sends a message that panics, root supervisor sends `Terminate`, and the test asserts that `on_exit` received `ExitCode::Terminated` (not `Dropped`).

---

### P0#2: Root supervision loop hits `unreachable!()` on valid signals

**File:** `theta/src/context.rs` lines 320–355  
**Real bug?** Yes — confirmed by failing test.

**Problem:**  
`RootContext::default()` spawns a supervision task that matches on incoming `RawSignal`. The original code only handled `Escalation` and `ChildDropped`, with a catch-all `_ => unreachable!("Escalation and ChildDropped are the only signals expected")`. But `Terminate`, `Pause`, `Resume`, `Restart`, and `Monitor` are all valid signals that can be received — for example, calling `root.terminate()` on a root-spawned actor immediately panics the supervision task.

**Fix:**  
Replaced the catch-all with explicit match arms:

- `Terminate(k)` — sends `Terminate` to all tracked children, acknowledges
- `Pause(k)` / `Resume(k)` / `Restart(k)` — acknowledge (no-op for root; root has no state to pause/restart)
- `Monitor(_)` — ignored (root context doesn't support monitoring)
- `Escalation` — logs error and terminates the offending child (previously used `.unwrap()` which is P1#7)
- `ChildDropped` — removes child from tracking set (unchanged)

**Test:** `p0_2_root_context_terminate_should_not_panic` — spawns an actor from `RootContext`, sends `Terminate` signal, and asserts the root supervision loop doesn't panic.

---

### P0#3: `Deadline` `IntoFuture` generics prevent `ask().timeout().await` from compiling

**File:** `theta/src/actor_ref.rs` lines 1315–1332  
**Real bug?** Yes — `ask().timeout().await` does not compile.

**Problem:**  
The original impl:
```rust
impl<'a, R, M> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<M, RequestError<M>>> + Send,
```
This requires the **success type** and the **error's type parameter** to be the same `M`. But `MsgRequest::IntoFuture` produces:
```
Result<M::Return, RequestError<MsgPack<A>>>
```
Here the success type is `M::Return` and the error wraps `MsgPack<A>` — these are fundamentally different types, so the bound `Result<M, RequestError<M>>` can never be satisfied. Any `actor.ask(msg).timeout(dur).await` is a compile error (E0277).

**Current fix:**  
Introduced `DeadlineError` enum (single variant `Timeout`) and rewrote the impl:
```rust
impl<'a, R, T, E> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<T, E>> + Send,
    R::IntoFuture: Send,
    E: From<DeadlineError>,
```
Plus `impl<T> From<DeadlineError> for RequestError<T>`.

**See the Deadline redesign analysis below for alternatives.**

**Test:** `p0_3_ask_timeout_should_compile_and_work` — calls `actor.ask(msg).timeout(1s).await` and asserts the response.

---

### P0#4: `panic_msg()` hits `unreachable!()` on non-string panic payloads

**File:** `theta/src/base.rs` line 97  
**Real bug?** Yes — confirmed by failing test.

**Problem:**  
The `panic_msg()` utility extracts a human-readable string from a `Box<dyn Any + Send>` panic payload. It checks for `&str` and `String`, then calls `unreachable!()`. But Rust allows `panic_any(42u32)` or any arbitrary type — a non-string panic payload causes the supervision task to panic itself, silently killing the actor runtime. The actor's `on_exit` callback is never invoked.

**Fix:**  
`unreachable!()` → `"<non-string panic payload>".to_string()`, providing a safe fallback.

**Test:** `p0_4_panic_msg_should_handle_non_string_payloads` — spawns an actor whose message handler calls `panic_any(42u32)`, and verifies the actor completes its lifecycle normally (on_exit fires).

---

## P1 — Suspected Bugs (Investigated — Not Real)

### P1#5: `sig_rx.recv().unwrap()` could panic if all SigTx senders are dropped

**File:** `theta/src/actor_instance.rs` — `wait_signal()` method  
**Real bug?** No.

**Analysis:**  
`sig_rx.recv()` returns `Err` only when all `SigTx` senders for that channel are dropped. But every actor holds `this_hdl: ActorHdl(SigTx)` — a sender to its **own** signal channel. This sender is stored in the actor's config and is only dropped when the actor's config is dropped, which happens *after* `wait_signal()` has returned and the actor's main loop has exited. Therefore `sig_rx.recv()` can never return `Err` while the actor is alive and calling `wait_signal()`.

**Test:** `p1_5_sig_rx_unwrap_cannot_be_triggered` — terminates an actor cleanly and verifies no panic occurs, confirming the invariant holds.

---

### P1#6: `parent_hdl.raw_send(ChildDropped).expect()` could panic if parent is already dropped

**File:** `theta/src/actor_instance.rs` — `Drop` impl  
**Real bug?** No.

**Analysis:**  
When a child actor is dropped, `Drop::drop()` sends `RawSignal::ChildDropped` to the parent via `parent_hdl.raw_send().expect("parent is dead")`. The concern was: what if the parent has already been dropped, closing its signal channel?

This cannot happen due to a channel-lifetime invariant:
1. The child holds `parent_hdl: ActorHdl(SigTx)` — a **sender** to the parent's signal channel
2. The parent enters `Cont::Drop` only when `sig_rx.sender_count() <= DROP_HDL_COUNT`
3. Since the child holds a sender, the parent's sender count stays above the threshold
4. Therefore the parent **cannot** enter `Cont::Drop` (and close its channel) while any child is alive

The parent can only be terminated via `Cont::Terminate`, but the `terminate()` code path signals all children first and waits for them to exit. By the time the parent's channel closes, no child is alive to send `ChildDropped`.

**Test:** `p1_6_drop_lifecycle_child_dropped_send_works` — parent spawns a child, parent ref is dropped, verifies the whole lifecycle completes without panic.

---

### P1#7: Root escalation handler `.unwrap()` could panic

**File:** `theta/src/context.rs` — root supervision loop  
**Real bug?** Not independently — already fixed in P0#2.

**Analysis:**  
The root supervision loop previously called `.unwrap()` when sending a `Terminate` signal back to an escalating child. The concern was: what if the child is already dead? But escalation signals are sent by the supervision task *from within* the child's own lifecycle — the child is alive (in `wait_signal()`) when the root processes the escalation. The `unwrap()` could never fail under normal conditions.

Regardless, the P0#2 fix replaced it with `if let Err(e)` for defensive robustness.

**Test:** `p1_7_root_escalation_handling_works` — triggers a panic in a root-spawned actor and verifies the root supervision loop handles the escalation without panicking.

---

## Deadline Redesign Analysis (P0#3)

### Why the current fix feels like monkey-patching

The current fix introduces `DeadlineError` — a new public enum with a single variant `Timeout` — that exists solely as a bridge for type conversion. The flow is:

```
tokio timeout elapsed → DeadlineError::Timeout → From<DeadlineError> → RequestError::Timeout
```

Issues:
1. **Unnecessary indirection** — `DeadlineError` is a one-variant enum whose only purpose is to be converted into something else
2. **Viral trait bound** — `E: From<DeadlineError>` is now baked into `Deadline`'s `IntoFuture` impl, meaning anyone using `Deadline` with a custom error type must implement `From<DeadlineError>`
3. **Not integrated** — `DeadlineError` is not in the prelude, not part of the API vocabulary, and feels bolted on
4. **Over-generic** — `Deadline` is treated as a general-purpose timeout wrapper (`R: IntoFuture<Output = Result<T, E>>`), but it's actually only constructable from `MsgRequest::timeout()` and `SignalRequest::timeout()`

### The key insight

`Deadline` has **private fields** and **no public constructor**. It can only be created by:
- `MsgRequest::timeout()` → error type is `RequestError<MsgPack<A>>`
- `SignalRequest::timeout()` → error type is `RequestError<RawSignal>`

Both callers produce `RequestError<_>`. There is no third use case.

### Option A — Bind directly to `RequestError` (recommended)

```rust
impl<'a, R, T, S> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<T, RequestError<S>>> + Send,
    R::IntoFuture: Send,
{
    type Output = Result<T, RequestError<S>>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            match tokio::time::timeout(self.duration, self.request).await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(RequestError::Timeout),
            }
        })
    }
}
```

**Changes needed:**
- Delete `DeadlineError` enum entirely
- Delete `impl<T> From<DeadlineError> for RequestError<T>`
- Replace `IntoFuture` impl with the above

**Pros:**
- Zero new types — uses `RequestError::Timeout` directly (which already exists)
- Simpler generics — `T` (success) and `S` (send-error payload) are clearly separated
- Matches reality — `Deadline` is already tightly coupled to the actor system
- No trait bound leakage — no `From<X>` required from error types

**Cons:**
- `Deadline` cannot be used with non-`RequestError` futures
- This is not a real con since `Deadline` has no public constructor and is already actor-system-internal

### Option B — Keep generic `E` but use `tokio::time::error::Elapsed`

```rust
impl<'a, R, T, E> IntoFuture for Deadline<'a, R>
where
    R: 'a + IntoFuture<Output = Result<T, E>> + Send,
    R::IntoFuture: Send,
    E: From<tokio::time::error::Elapsed>,
```
Plus `impl<T> From<Elapsed> for RequestError<T>`.

Same pattern as current fix but substitutes `tokio::time::error::Elapsed` for `DeadlineError`. Eliminates the custom type but still has the viral `From` bound, and now leaks a tokio implementation detail into the public API. **Worse than the current fix.**

### Option C — Concrete timeout types per request

```rust
pub struct MsgRequestWithTimeout<'a, A, M> { ... }
pub struct SignalRequestWithTimeout<'a> { ... }
```

Each with its own `IntoFuture` impl that knows the exact output type.

**Pros:** No generics needed at all  
**Cons:** Code duplication — the timeout logic is identical, just wrapped around different inner futures. Makes the API surface larger for no benefit.

### Option D — Nested `Result` wrapper

```rust
impl<'a, R> IntoFuture for Deadline<'a, R>
where R: 'a + IntoFuture + Send, R::IntoFuture: Send,
{
    type Output = Result<R::Output, DeadlineError>;
```

This changes the return type to `Result<Result<T, E>, DeadlineError>`, requiring nested error handling:
```rust
match actor.ask(msg).timeout(dur).await {
    Ok(Ok(value)) => ...,
    Ok(Err(request_err)) => ...,
    Err(DeadlineError::Timeout) => ...,
}
```
**Worse ergonomics.** The current flat `Result<T, E>` API is better.

### Recommendation

**Option A** — bind `Deadline` directly to `RequestError`. It:
- Removes `DeadlineError` entirely (net negative lines)
- Uses the existing `RequestError::Timeout` variant directly
- Has simpler, more readable type bounds
- Correctly reflects the actual coupling (private constructor, actor-only usage)
- Requires zero downstream changes (test stays the same, types unify the same way)
