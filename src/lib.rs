//! This crate provides a rate limiter that follows a token-bucket approach,
//! while taking the runtime of the task into account.
//! Convenient for calling strict rate-limited APIs, Ritlers can be used
//! to ensure that even when routing/caching inconsistencies happen, your
//! requests will never accidentally surpass the rate limit.
//! This is done by waiting for the task to finish(wait for a response) before
//! it starts to schedule the next task.
//!
//! This crate contains both a blocking and an async implementation, each
//! available under their respective feature flag (default = blocking)
#[cfg(feature = "blocking")]
pub mod blocking;

#[cfg(feature = "async")]
pub mod async_rt;
