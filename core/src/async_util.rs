//! Tiny shared async utilities. Currently just [`BoxFuture`] — kept as its
//! own module (rather than defined once inside `ai::traits` and quietly
//! reused nowhere else) because it isn't AI-specific: `ai::traits::LlmProvider`
//! and `scraping::browser::BrowserDriver` both need "a boxed, pinned,
//! `Send` future" for their trait methods, and neither should have to depend
//! on the other's module just to reuse this one type alias.

use std::future::Future;
use std::pin::Pin;

/// A boxed, pinned future — used by trait methods that can't be `async fn`
/// directly (traits with `dyn`-compatible async methods need this until
/// Rust's native support for that stabilizes further).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
