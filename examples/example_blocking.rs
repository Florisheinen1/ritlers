use std::time::{Duration, Instant};

use ritlers::blocking::RateLimiter;

fn main() {
	// 2 tasks per second: tasks 1 and 2 run immediately, then the limiter
	// waits before starting task 3.  schedule_task returns the Instant the
	// task finished, so we can show exactly how much wall-clock time elapsed.
	let mut ratelimiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
	let start = Instant::now();

	let done = ratelimiter.schedule_task(|| println!("Task 1 executed"));
	println!("  → Task 1 finished at +{:.2?}", done.duration_since(start));

	let done = ratelimiter.schedule_task(|| println!("Task 2 executed"));
	println!("  → Task 2 finished at +{:.2?}", done.duration_since(start));

	let done = ratelimiter.schedule_task(|| println!("Task 3 executed"));
	println!("  → Task 3 finished at +{:.2?}", done.duration_since(start));

	let done = ratelimiter.schedule_task(|| println!("Task 4 executed"));
	println!("  → Task 4 finished at +{:.2?}", done.duration_since(start));
}
