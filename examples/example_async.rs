#![cfg(feature = "async")]
use ritlers::async_rt::RateLimiter;

use std::time::Duration;

// 2 tasks per second, 6 tasks total:
//   tasks 1-2 are in the first burst  (ETA ≈ 0s)
//   tasks 3-4 are in the second burst (ETA ≈ 1s)
//   tasks 5-6 are in the third burst  (ETA ≈ 2s)
//
// All 6 are scheduled concurrently, so the ETA for each reflects how deep
// in the queue it landed at the moment schedule_task was called.
#[tokio::main]
async fn main() {
	let limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();

	let handles: Vec<_> = (1..=6)
		.map(|i| {
			let limiter = limiter.clone();
			tokio::spawn(async move {
				let eta = limiter
					.schedule_task(async move {
						println!("Task {i} executed");
					})
					.await;
				println!("  → Task {i} scheduled, estimated wait: {eta:.1?}");
			})
		})
		.collect();

	for handle in handles {
		handle.await.unwrap();
	}
}
