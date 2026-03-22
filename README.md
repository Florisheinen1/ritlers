# Ritlers

A task-length aware rate limiter for Rust, useful for calling APIs that apply strict rate limiting.

## Why ritlers?

Most rate limiters count when a task _starts_. Because network requests have variable latency, and due to the possibility of caching in underlying libraries, this can still cause too large bursts to arrive at a downstream server simultaneously. Ritlers instead waits for each task to _finish_ before its slot becomes available again, so the rate limit is respected regardless of how long individual requests take.

When tasks complete instantly the behaviour is identical to a standard token-bucket limiter.

## Features

- **Blocking** (`blocking` — default): parks the calling thread until a slot is free
- **Async** (`async`): non-blocking, backed by Tokio
- **Retry support**: return `TaskResult::TryAgain` from a scheduled task to re-queue it ahead of newly scheduled tasks, or `TaskResult::Done` to stop

## Usage

### Blocking

`schedule_task` blocks the calling thread until a slot is free and returns the `Instant` the task finished.

```rust
// 2 requests per second
let mut limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

let finished_at = limiter.schedule_task(|| {
    // perform API call
});

// Retry on rate-limit errors; stop otherwise
limiter.schedule_task_with_retry(|| {
    match do_api_call() {
        Ok(http_200)  => TaskResult::Done,
        Err(http_429) => TaskResult::TryAgain,
        Err(_)        => TaskResult::Done,
    }
});
```

### Async

`schedule_task` and `schedule_task_with_retry` both return a `Duration` estimating how long until the task starts, based on the current queue depth. This can be shown to users or used for logging.

```rust
// 2 requests per second
let limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

let eta = limiter.schedule_task(async {
   // perform API call
}).await;
println!("task started after an estimated {eta:?}");

// Retry on rate-limit errors; stop otherwise
limiter.schedule_task_with_retry(|| async {
   match do_api_call().await {
      Ok(_)           => TaskResult::Done,
      Err(http_429)   => TaskResult::TryAgain,
      Err(_)          => TaskResult::Done,
   }
}).await;
```

## `TaskResult`

| Variant    | Meaning                                                    |
| ---------- | ---------------------------------------------------------- |
| `Done`     | Task is finished (any outcome); release the slot           |
| `TryAgain` | Re-queue ahead of newly scheduled tasks and retry          |
