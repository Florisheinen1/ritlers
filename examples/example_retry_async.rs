#![cfg(feature = "async")]
use std::{sync::Arc, time::Duration};

use ritlers::{TaskResult, async_rt::RateLimiter};
use tokio::sync::Mutex;

/// Simulates an API call that is rate-limited on the first two attempts
/// and succeeds on the third.
async fn simulated_api_call(attempt: u32) -> Result<String, String> {
	if attempt < 3 {
		Err(format!("429 Too Many Requests (attempt {})", attempt))
	} else {
		Ok(format!("200 OK (attempt {})", attempt))
	}
}

#[tokio::main]
async fn main() {
	// 2 requests per second
	let rate_limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

	// Each task gets its own retry budget of 3 attempts.
	// The factory is Fn (not FnMut), so shared state is wrapped in Arc<Mutex>.
	for task_id in 1u32..=3 {
		let retries_remaining = Arc::new(Mutex::new(3u32));
		let attempt = Arc::new(Mutex::new(0u32));

		rate_limiter
			.schedule_task_with_retry(move || {
				let retries_remaining = retries_remaining.clone();
				let attempt = attempt.clone();
				async move {
					let mut attempt = attempt.lock().await;
					*attempt += 1;
					let current_attempt = *attempt;

					match simulated_api_call(current_attempt).await {
						Ok(response) => {
							println!("[task {task_id}] {response}");
							TaskResult::Success
						}
						Err(e) => {
							let mut retries = retries_remaining.lock().await;
							if *retries > 0 {
								*retries -= 1;
								println!(
									"[task {task_id}] {e} — {} retries remaining",
									*retries
								);
								TaskResult::TryAgain
							} else {
								println!("[task {task_id}] {e} — giving up");
								TaskResult::Success
							}
						}
					}
				}
			})
			.await;
	}

	// Wait for all scheduled tasks (including retries) to finish
	tokio::time::sleep(Duration::from_secs(10)).await;
}
