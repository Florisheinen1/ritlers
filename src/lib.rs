//! A task-length aware rate limiter.
//!
//! Most rate limiters count when a task *starts*. Because network requests have
//! variable latency, this can still cause bursts to arrive at a downstream API
//! simultaneously. **Ritlers** instead waits for each task to *finish* before
//! its slot becomes available again, so the rate limit is respected regardless
//! of how long individual requests take.
//!
//! When tasks complete instantly the behaviour is identical to a standard
//! token-bucket limiter.
//!
//! # Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `blocking` | yes | Synchronous [`blocking::RateLimiter`] that parks the calling thread |
//! | `async` | no | Async [`async_rt::RateLimiter`] backed by Tokio |
//!
//! # Quick start
//!
//! ## Blocking
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use ritlers::blocking::RateLimiter;
//! use ritlers::TaskResult;
//!
//! let mut limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
//!
//! // Schedule a plain task
//! limiter.schedule_task(|| { /* perform API call */ });
//!
//! // Schedule a task that retries on rate-limit errors
//! limiter.schedule_task_with_retry(|| {
//!     match do_api_call() {
//!         Ok(_)  => TaskResult::Done,
//!         Err(_) => TaskResult::TryAgain,
//!     }
//! });
//! # fn do_api_call() -> Result<(), ()> { Ok(()) }
//! ```
//!
//! ## Async
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use ritlers::async_rt::RateLimiter;
//! use ritlers::TaskResult;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
//!
//! // Schedule a plain task
//! limiter.schedule_task(async { /* perform API call */ }).await;
//!
//! // Schedule a task that retries on rate-limit errors
//! limiter.schedule_task_with_retry(|| async {
//!     match do_api_call().await {
//!         Ok(_)  => TaskResult::Done,
//!         Err(_) => TaskResult::TryAgain,
//!     }
//! }).await;
//! # async fn do_api_call() -> Result<(), ()> { Ok(()) }
//! # }
//! ```

#[cfg(feature = "blocking")]
pub mod blocking;

#[cfg(feature = "async")]
pub mod async_rt;

/// The outcome of a retryable task.
///
/// Return this from the closure passed to `schedule_task_with_retry`.
/// When [`TryAgain`](TaskResult::TryAgain) is returned, the task is
/// re-queued and will run again as soon as the next rate-limit slot is free.
/// Retried tasks are prioritised over newly scheduled tasks.
/// When [`Done`](TaskResult::Done) is returned, the task stops retrying
/// and the slot is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskResult {
	/// The task is done (successfully or not); release the slot.
	Done,
	/// The task should be retried at the next available slot.
	TryAgain,
}

/// Error returned by [`blocking::RateLimiter::new`] and
/// [`async_rt::RateLimiter::new`] when `amount` is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroAmountError;

impl std::fmt::Display for ZeroAmountError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("amount must be greater than zero")
	}
}

impl std::error::Error for ZeroAmountError {}
