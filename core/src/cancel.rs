//! Cooperative cancellation for long-running AI operations.
//!
//! The user-visible motivation: "Analyze this Page" and friends can take
//! anywhere from a few seconds (small local model, GPU) to *hours*
//! (mis-sized local model that silently fell back to CPU). Without
//! cancellation the only recovery is to force-kill the whole app, losing
//! whatever else was open. A cancel button on the progress toast keeps
//! the user in control regardless of how badly the model is behaving.
//!
//! # Shape
//! [`CancellationToken`] is a cheap `Clone`-able handle around an
//! `Arc<AtomicBool>` plus a `Notify`. Callers **check** it at safe points
//! (before spawning subwork, in a `tokio::select!` branch); the requester
//! (UI, Tauri command) **triggers** it. Once triggered, all clones stay
//! triggered forever — cancellation is a one-way latch, deliberately not
//! reusable, so a stale `cancel()` from an earlier operation can never
//! silently kill a fresh one.
//!
//! We deliberately don't pull in `tokio-util`'s `CancellationToken` here:
//! this is ~40 lines of totally standard code, adding a dep for it
//! doubles the audit surface for one type.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// A one-way "please stop" latch shared between the operation and its
/// canceller. Cheap to clone. See module docs.
#[derive(Clone)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancellationToken {
    /// Fresh, un-cancelled token.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    /// A permanently-un-cancelled token, for call sites that don't care
    /// about cancellation (batch imports, tests, the CLI). Same shape as
    /// [`new`](Self::new); the name documents intent at the call site.
    pub fn disabled() -> Self {
        Self::new()
    }

    /// Trigger the latch. Safe to call from any thread, and safe to call
    /// multiple times (subsequent calls are no-ops).
    pub fn cancel(&self) {
        // `Release` so that any state a canceller mutated *before* this
        // call (e.g. UI state, a log line) is visible to observers that
        // load with `Acquire`. `Notify` guarantees at-least-one wakeup.
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    /// Non-blocking "has anyone triggered this yet?" check. Use in
    /// synchronous code / at loop tops. For async waiters that want to
    /// race the cancellation itself, use [`cancelled`](Self::cancelled).
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Resolves the first time the token is cancelled. Never resolves if
    /// the token stays live — designed to be a `select!` branch, not
    /// something to `.await` on its own. If the token is *already*
    /// cancelled when this is called, it resolves immediately.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let waiter = self.inner.notify.notified();
        tokio::pin!(waiter);
        // The between-check-and-await window is why we re-check after
        // registering: if the canceller ran between our `is_cancelled`
        // check above and the `notified()` registration, the notify
        // wakeup already fired for nobody and we'd hang forever.
        if self.is_cancelled() {
            return;
        }
        waiter.await;
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationToken")
            .field("is_cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fresh_token_is_not_cancelled() {
        let t = CancellationToken::new();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn cancel_sets_the_flag_visibly_to_clones() {
        let a = CancellationToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled(), "clones must observe cancellation");
    }

    #[tokio::test]
    async fn cancelled_future_resolves_when_cancel_is_called() {
        let t = CancellationToken::new();
        let t2 = t.clone();
        let handle = tokio::spawn(async move { t2.cancelled().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        t.cancel();
        tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("cancelled() should resolve within timeout")
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_resolves_immediately_when_already_cancelled() {
        // Regression for the classic race: canceller runs before the
        // waiter registers with `notified()`. If we didn't re-check the
        // atomic after registration, the waiter would hang forever
        // because the notify wakeup already fired for nobody.
        let t = CancellationToken::new();
        t.cancel();
        tokio::time::timeout(Duration::from_millis(50), t.cancelled())
            .await
            .expect("already-cancelled cancelled() must resolve fast");
    }
}
