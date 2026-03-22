use std::time::{Duration, Instant};

use crate::{TaskResult, ZeroAmountError};

/// A synchronous, task-length aware rate limiter.
///
/// Schedules tasks one at a time on the calling thread, blocking until a
/// rate-limit slot is free before each execution. The slot is only released
/// after the task *finishes*, ensuring downstream services never receive more
/// concurrent requests than `amount` within any `per_time` window.
///
/// # Example
///
/// ```rust,no_run
/// use std::time::Duration;
/// use ritlers::blocking::RateLimiter;
///
/// // Allow up to 2 requests per second
/// let mut limiter = RateLimiter::new(2, Duration::from_secs(1)).unwrap();
/// limiter.schedule_task(|| { /* perform API call */ });
/// ```
pub struct RateLimiter {
	finish_times: Vec<Instant>, // Sorted oldest-first list of task finish times
	per_time: Duration,
}

impl RateLimiter {
	/// Creates a new rate limiter that allows `amount` tasks per `per_time`.
	///
	/// # Errors
	///
	/// Returns [`Err(ZeroAmountError)`](ZeroAmountError) if `amount` is zero.
	///
	/// # Example
	///
	/// ```rust
	/// use std::time::Duration;
	/// use ritlers::blocking::RateLimiter;
	///
	/// let limiter = RateLimiter::new(5, Duration::from_secs(1)).unwrap();
	/// ```
	pub fn new(amount: usize, per_time: Duration) -> Result<Self, ZeroAmountError> {
		if amount == 0 {
			return Err(ZeroAmountError);
		}
		Ok(Self {
			finish_times: vec![Instant::now(); amount],
			per_time,
		})
	}

	/// Runs the given task, blocking the calling thread until a rate-limit slot
	/// is available.
	///
	/// Returns the [`Instant`] when the task finished.
	///
	/// # Example
	///
	/// ```rust,no_run
	/// # use std::time::Duration;
	/// # use ritlers::blocking::RateLimiter;
	/// # let mut rate_limiter = RateLimiter::new(1, Duration::from_secs(1)).unwrap();
	/// let finished_at = rate_limiter.schedule_task(|| {
	///     // perform API call
	/// });
	/// ```
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
		// Insert at the front to keep the list sorted oldest-first
		self.finish_times.insert(0, finished_at);
		finished_at
	}

	/// Runs the given task, retrying it after each rate-limit interval until it
	/// returns [`TaskResult::Done`].
	///
	/// Each attempt — including retries — must wait for an available slot, so
	/// the rate limit is respected throughout. Blocks the calling thread until
	/// the task is done.
	///
	/// Returns the [`Instant`] when the final attempt finished.
	///
	/// # Example
	///
	/// ```rust,no_run
	/// # use std::time::Duration;
	/// # use ritlers::{blocking::RateLimiter, TaskResult};
	/// # let mut rate_limiter = RateLimiter::new(1, Duration::from_secs(1)).unwrap();
	/// rate_limiter.schedule_task_with_retry(|| {
	///     match do_api_call() {
	///         Ok(_)  => TaskResult::Done,
	///         Err(_) => TaskResult::TryAgain,
	///     }
	/// });
	/// # fn do_api_call() -> Result<(), ()> { Ok(()) }
	/// ```
	pub fn schedule_task_with_retry<F>(&mut self, mut task: F) -> Instant
	where
		F: FnMut() -> TaskResult,
	{
		loop {
			let oldest_finish_time = self.finish_times.pop().expect("At least one finish time");

			let now = Instant::now();

			let wait_until = oldest_finish_time + self.per_time;
			let time_to_wait = wait_until.duration_since(now);
			std::thread::sleep(time_to_wait);

			let result = task();

			let finished_at = Instant::now();
			self.finish_times.insert(0, finished_at);

			match result {
				TaskResult::Done => return finished_at,
				TaskResult::TryAgain => continue,
			}
		}
	}
}

#[cfg(test)]
mod tests {

	use std::time::{Duration, Instant};

	use crate::TaskResult;
	use crate::blocking::RateLimiter;

	#[test]
	fn test_parameter_correctness() {
		assert!(
			RateLimiter::new(0, Duration::from_secs(1)).is_err(),
			"Zero tasks allowed is not possible"
		);
		assert!(RateLimiter::new(1, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(2, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(500, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(1, Duration::ZERO).is_ok());
		assert!(RateLimiter::new(1, Duration::from_secs(500)).is_ok());
		assert!(RateLimiter::new(1, Duration::MAX).is_ok());
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
		// Tasks 2 and 3 share the initial burst, so they finish close to task 1
		assert!(finished_2.duration_since(finished_1) < Duration::from_secs(1));
		assert!(finished_3.duration_since(finished_1) < Duration::from_secs(1));
		// Task 4 must wait for the window opened by task 1 to expire
		assert!(finished_1 + Duration::from_secs(1) < finished_4);
	}

	#[test]
	fn test_retry() {
		let mut rate_limiter = RateLimiter::new(1, Duration::from_millis(50)).unwrap();

		let attempts = std::cell::Cell::new(0u32);
		let finished = rate_limiter.schedule_task_with_retry(|| {
			attempts.set(attempts.get() + 1);
			if attempts.get() < 3 {
				TaskResult::TryAgain
			} else {
				TaskResult::Done
			}
		});

		assert_eq!(
			attempts.get(),
			3,
			"Should have attempted 3 times (2 retries)"
		);
		assert!(finished <= Instant::now());
	}

	/// Done on the very first attempt must stop the loop without retrying.
	#[test]
	fn test_done_stops_immediately() {
		let mut rate_limiter = RateLimiter::new(1, Duration::from_millis(50)).unwrap();

		let attempts = std::cell::Cell::new(0u32);
		rate_limiter.schedule_task_with_retry(|| {
			attempts.set(attempts.get() + 1);
			TaskResult::Done
		});

		assert_eq!(
			attempts.get(),
			1,
			"Done should stop after a single attempt without retrying"
		);
	}

	/// With per_time = 0 there is no enforced wait between tasks, so many
	/// tasks should complete near-instantly with no sleeping.
	#[test]
	fn test_zero_per_time() {
		let mut rate_limiter = RateLimiter::new(1, Duration::ZERO).unwrap();

		let start = Instant::now();
		for _ in 0..5 {
			rate_limiter.schedule_task(|| {});
		}
		assert!(
			start.elapsed() < Duration::from_millis(200),
			"Tasks with per_time=0 should complete without enforced delays"
		);
	}

	/// Retries must respect the same rate-limit interval as plain tasks — they
	/// are not allowed to run back-to-back without waiting for a slot.
	#[test]
	fn test_retry_is_rate_limited() {
		let mut rate_limiter = RateLimiter::new(1, Duration::from_millis(100)).unwrap();

		let finish_times = std::cell::RefCell::new(Vec::<Instant>::new());
		rate_limiter.schedule_task_with_retry(|| {
			finish_times.borrow_mut().push(Instant::now());
			if finish_times.borrow().len() < 3 {
				TaskResult::TryAgain
			} else {
				TaskResult::Done
			}
		});

		let times = finish_times.borrow();
		assert_eq!(times.len(), 3);
		assert!(
			times[1].duration_since(times[0]) >= Duration::from_millis(100),
			"Second attempt should be at least one interval after the first"
		);
		assert!(
			times[2].duration_since(times[1]) >= Duration::from_millis(100),
			"Third attempt should be at least one interval after the second"
		);
	}

	/// The internal `finish_times` vec must always hold exactly `amount`
	/// entries — never more, never fewer — regardless of how many tasks run.
	#[test]
	fn test_finish_times_count_stays_constant() {
		let amount = 3;
		let mut rate_limiter = RateLimiter::new(amount, Duration::ZERO).unwrap();

		assert_eq!(rate_limiter.finish_times.len(), amount);
		for _ in 0..5 {
			rate_limiter.schedule_task(|| {});
			assert_eq!(
				rate_limiter.finish_times.len(),
				amount,
				"finish_times length must stay equal to amount after each task"
			);
		}
	}

	/// Done after several TryAgain returns must stop the loop at that point.
	#[test]
	fn test_try_again_then_done() {
		let mut rate_limiter = RateLimiter::new(1, Duration::from_millis(50)).unwrap();

		let attempts = std::cell::Cell::new(0u32);
		rate_limiter.schedule_task_with_retry(|| {
			attempts.set(attempts.get() + 1);
			if attempts.get() < 3 {
				TaskResult::TryAgain
			} else {
				TaskResult::Done
			}
		});

		assert_eq!(
			attempts.get(),
			3,
			"Should have run 3 times (2 TryAgain retries) before stopping on Done"
		);
	}
}
