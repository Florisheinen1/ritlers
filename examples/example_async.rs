#![cfg(feature = "async")]
use ritlers::async_rt::RateLimiter;

use std::time::Duration;
use tokio;

#[tokio::main]
async fn main() {
	let ratelimiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
	ratelimiter
		.schedule_task(async {
			println!("Task 1 executed immediately");
		})
		.await;
	ratelimiter
		.schedule_task(async {
			println!("Task 2 executed immediately");
		})
		.await;
	ratelimiter
		.schedule_task(async {
			println!("Task 3 had to wait a bit");
		})
		.await;
	ratelimiter
		.schedule_task(async {
			println!("Task 4 can go immediately afterwards");
		})
		.await;

	tokio::time::sleep(Duration::from_secs(3)).await;
}
