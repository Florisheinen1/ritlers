# RITLERS

A task-length aware rate limiter, made in Rust.
Useful for calling APIs that apply strict rate limiting.

## Difference from other rate limiters

Most other rate limiters limit the start of each provided task. Unfortunately, due to network routing inconsistencies, even when requests are periodically sent conforming the rate limit, too many requests might still end up at the endpoint simultaneously. Ritlers therefore waits until the provided tasks are "done" (E.g. received a response) before rate limit time continues.

This does mean scheduled tasks are precisely run less frequently than the rate limit requires by the amount of time the tasks require to run. When tasks are finished instantly, this rate limiter exactly matches behavior of any other token-bucket rate limiter.

## Example of code

```rs
// Allow 2 tasks per 3 seconds
let ritlers = RateLimiter::new(2, Duration::from_secs(3));
ritlers.schedule_task(Box::new(move || {
	Box::pin(async move {
		// First task started
		std::thread::sleep(Duration::from_secs(3));
		// First task ended
	})
}));
```
