# Ritlers

A task-length aware rate limiter for Rust, useful for calling APIs that apply strict rate limiting.

## Why ritlers?

Most rate limiters count when a task *starts*. Because network requests have variable latency, this can still cause bursts to arrive at a downstream server simultaneously. Ritlers instead waits for each task to *finish* before its slot becomes available again, so the rate limit is respected regardless of how long individual requests take.

When tasks complete instantly the behaviour is identical to a standard token-bucket limiter.

## Features

- **Blocking** (`blocking` — default): parks the calling thread until a slot is free
- **Async** (`async`): non-blocking, backed by Tokio
- **Retry support**: return `TaskResult::TryAgain` from a task to re-queue it at the front of the queue

## Usage

### Blocking

```rust
use std::time::Duration;
use ritlers::blocking::RateLimiter;
use ritlers::TaskResult;

// 2 requests per second
let mut limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

limiter.schedule_task(|| {
    // perform API call
});

// Retry automatically on rate-limit errors
limiter.schedule_task_with_retry(|| {
    match do_api_call() {
        Ok(_)  => TaskResult::Success,
        Err(_) => TaskResult::TryAgain,
    }
});
```

### Async

```rust
use std::time::Duration;
use ritlers::async_rt::RateLimiter;
use ritlers::TaskResult;

#[tokio::main]
async fn main() {
    // 2 requests per second
    let limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

    limiter.schedule_task(async {
        // perform API call
    }).await;

    // Retry automatically on rate-limit errors
    limiter.schedule_task_with_retry(|| async {
        match do_api_call().await {
            Ok(_)  => TaskResult::Success,
            Err(_) => TaskResult::TryAgain,
        }
    }).await;
}
```

## Contribute

Contributions are welcome. Feel free to open an issue or create a PR.
