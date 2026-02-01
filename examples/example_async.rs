#![cfg(feature = "async")]
use ritlers::async_rt::RateLimiter;

use std::time::Duration;
use tokio::{
	self,
	sync::oneshot::{self, Receiver, error::TryRecvError},
};

#[tokio::main]
async fn main() {
	let mut ratelimiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
	ratelimiter
		.schedule_task(Box::new(move || {
			Box::pin(async move {
				println!("Task 1 executed immediately");
			})
		}))
		.await;
	ratelimiter
		.schedule_task(Box::new(move || {
			Box::pin(async move {
				println!("Task 2 executed immediately");
			})
		}))
		.await;
	ratelimiter
		.schedule_task(Box::new(move || {
			Box::pin(async move {
				println!("Task 3 had to wait a bit");
			})
		}))
		.await;
	ratelimiter
		.schedule_task(Box::new(move || {
			Box::pin(async move {
				println!("Task 4 can go immediately afterwards");
			})
		}))
		.await;

	// Wait until all tasks are done
	ratelimiter.wait_until_idle().await;
}
