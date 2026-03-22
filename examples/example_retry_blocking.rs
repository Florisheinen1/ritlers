use std::time::Duration;

use ritlers::{TaskResult, blocking::RateLimiter};

/// Simulates an API call that is rate-limited on the first two attempts
/// and succeeds on the third.
fn simulated_api_call(attempt: u32) -> Result<String, String> {
	if attempt < 3 {
		Err(format!("429 Too Many Requests (attempt {})", attempt))
	} else {
		Ok(format!("200 OK (attempt {})", attempt))
	}
}

fn main() {
	// 2 requests per second
	let mut rate_limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
	let start = std::time::Instant::now();

	// Each task gets its own retry budget of 3 attempts.
	// The closure is FnMut, so the counter can live on the stack.
	for task_id in 1..=3 {
		let mut retries_remaining = 3u32;
		let mut attempt = 0u32;

		let done = rate_limiter.schedule_task_with_retry(|| {
			attempt += 1;

			match simulated_api_call(attempt) {
				Ok(response) => {
					println!("[task {task_id}] {response}");
					TaskResult::Done
				}
				Err(e) if retries_remaining > 0 => {
					retries_remaining -= 1;
					println!("[task {task_id}] {e} — {retries_remaining} retries remaining");
					TaskResult::TryAgain
				}
				Err(e) => {
					println!("[task {task_id}] {e} — giving up");
					TaskResult::Done
				}
			}
		});
		println!("[task {task_id}] finished at +{:.2?}", done.duration_since(start));
	}
}
