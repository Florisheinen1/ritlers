use std::time::{Duration, Instant};

pub struct RateLimiter {
	finish_times: Vec<Instant>, // Sorted (old -> recent) list of finish times
	per_time: Duration,
}

impl RateLimiter {
	pub fn new(amount: usize, per_time: Duration) -> Result<Self, ()> {
		if amount == 0 {
			// Invalid amount of requests per time. Needs to be >= 1
			return Err(());
		}
		Ok(Self {
			finish_times: vec![Instant::now(); amount],
			per_time,
		})
	}

	/// Runs the given task.
	/// Blocks the calling thread untill the task can perform
	/// Returns the instant when the task finished
	pub fn schedule_task<F>(&mut self, task: F) -> Instant
	where
		F: FnOnce(),
	{
		let oldest_finish_time = self.finish_times.pop().expect("At least one finish time");

		let now = Instant::now();

		let wait_until = oldest_finish_time + self.per_time;
		let time_to_wait = wait_until.duration_since(now);
		std::thread::sleep(time_to_wait);

		task();

		let finished_at = Instant::now();
		// Insert at beginning to stay ordered
		self.finish_times.insert(0, finished_at);
		finished_at
	}
}

#[cfg(test)]
mod tests {

	use std::time::{Duration, Instant};

	use crate::blocking::RateLimiter;

	#[test]
	fn test_parameter_correctness() {
		// Test amount of tokens
		assert!(
			RateLimiter::new(0, Duration::from_secs(1)).is_err(),
			"Zero tasks allowed is not possible"
		);
		assert!(RateLimiter::new(1, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(2, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(500, Duration::from_secs(1)).is_ok());
		// Test duration
		assert!(RateLimiter::new(1, Duration::ZERO).is_ok()); // Essentially no limit
		assert!(RateLimiter::new(1, Duration::from_secs(500)).is_ok());
		assert!(RateLimiter::new(1, Duration::MAX).is_ok()); // Allows only single task ever
	}

	#[test]
	fn test_initialization_finish_times() {
		let rate_limiter_1 = RateLimiter::new(1, Duration::from_secs(1)).unwrap();
		assert_eq!(
			rate_limiter_1.finish_times.len(),
			1,
			"There should be exactly one default finish time"
		);
		assert!(
			rate_limiter_1.finish_times.first().unwrap() < &Instant::now(),
			"The initial finish time should be older than now"
		);

		let rate_limiter_10 = RateLimiter::new(10, Duration::from_secs(10)).unwrap();
		assert_eq!(rate_limiter_10.finish_times.len(), 10);
		assert!(
			rate_limiter_10
				.finish_times
				.iter()
				.all(|t| t < &Instant::now())
		);
	}

	#[test]
	fn test_sequential_delays() {
		let mut rate_limiter = RateLimiter::new(1, Duration::from_secs(1)).unwrap();

		let finished_1 = rate_limiter.schedule_task(|| {
			std::thread::sleep(Duration::from_secs(1));
		});
		let finished_2 = rate_limiter.schedule_task(|| {
			std::thread::sleep(Duration::from_secs(1));
		});
		let finished_3 = rate_limiter.schedule_task(|| {
			std::thread::sleep(Duration::from_secs(1));
		});
		assert!(finished_1 + Duration::from_secs(1) < finished_2);
		assert!(finished_2 + Duration::from_secs(1) < finished_3);
	}

	#[test]
	fn test_concurrent_delays() {
		let mut rate_limiter = RateLimiter::new(3, Duration::from_secs(1)).unwrap();

		let finished_1 = rate_limiter.schedule_task(|| {});
		let finished_2 = rate_limiter.schedule_task(|| {});
		let finished_3 = rate_limiter.schedule_task(|| {});
		let finished_4 = rate_limiter.schedule_task(|| {});
		// Both task 2 and 3 should be done instantly after task 1
		assert!(finished_2.duration_since(finished_1) < Duration::from_secs(1));
		assert!(finished_3.duration_since(finished_1) < Duration::from_secs(1));
		// Task 4 however needs to wait for the time to elapse since task 1
		assert!(finished_1 + Duration::from_secs(1) < finished_4);
	}
}
