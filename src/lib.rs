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
//!     // Return TryAgain to re-queue at the next available slot
//!     TaskResult::Success
//! });
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
//!     // Return TryAgain to re-queue at the next available slot
//!     TaskResult::Success
//! }).await;
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
#[derive(Debug, PartialEq, Eq)]
pub enum TaskResult {
	/// The task completed successfully; release the slot.
	Success,
	/// The task should be retried at the next available slot.
	TryAgain,
}
