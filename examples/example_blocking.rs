use std::time::Duration;

use ritlers::blocking::RateLimiter;

fn main() {
	// This rate limiter allows for 2 tasks per 1 second,
	// which makes bursts of 2 tasks possible.
	let mut ratelimiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

	// Burst 1:
	ratelimiter.schedule_task(|| println!("Immediately executed 1!"));
	ratelimiter.schedule_task(|| println!("Immediately executed 2!"));
	// Pause before burst 2:
	ratelimiter.schedule_task(|| println!("Executed after 1 seconds!"));
	ratelimiter.schedule_task(|| println!("Executed immediately after!"));
}
