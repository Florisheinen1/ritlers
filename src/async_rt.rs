use std::{
	future::Future,
	pin::Pin,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};

use tokio::{
	sync::{Notify, Semaphore, mpsc},
	time::sleep,
};

type Task = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Task-length aware rate limiter
///
/// # Example:
/// ```rs
/// // Limit 3 tasks per 5 seconds
/// let ritlers = RateLimiter::new(3, Duration::from_secs(5));
/// let schedule_wait_time = ritlers
/// 	.schedule_task(Box::new(move || {
/// 		Box::pin(async move {
/// 			// Run task...
/// 		})
/// 	}));
/// ```
pub struct RateLimiter {
	task_queue_sender: mpsc::Sender<Task>,
	queued_tasks: Arc<AtomicUsize>,
	/// Triggers each time the queue gets empty
	/// Useful for waiting for all tasks to finish
	notify_idle: Arc<Notify>,

	amount: usize,
	per_time: Duration,
}

impl RateLimiter {
	/// Creates a new task-length aware rate limiter
	///
	/// `amount`: How many tasks run simultaneously \
	/// `per_time`: Per how many seconds
	///
	/// # Example
	///
	/// ```rs
	/// let limiter = RateLimiter::new(3, Duration::from_secs(4));
	/// ```
	pub fn new(amount: usize, per_time: Duration) -> Result<Self, ()> {
		if amount == 0 {
			return Err(());
		}

		// The semaphore indicates the maximum amount of concurrent tasks running
		let semaphore = Arc::new(Semaphore::new(amount));

		// Create the send and receive end of the Tasks channel
		let (sender, receiver) = mpsc::channel::<Task>(100);

		// Keep track of the size of the queue for ETA calculations
		let queue_size = Arc::new(AtomicUsize::new(0));
		let queue_size_upper = queue_size.clone();

		let notify_idle = Arc::new(Notify::new());
		let notify_idle_clone = notify_idle.clone();

		// Start running tasks
		tokio::spawn(async move {
			Self::run_tasks(
				receiver,
				semaphore,
				per_time,
				queue_size_upper,
				notify_idle_clone,
			)
			.await;
		});

		Ok(Self {
			task_queue_sender: sender,
			queued_tasks: queue_size,
			notify_idle,

			amount,
			per_time,
		})
	}

	/// Runs all scheduled tasks.
	/// Is thread blocking.
	/// Will run indefinitely
	async fn run_tasks(
		mut task_receiver: mpsc::Receiver<Task>,
		semaphore: Arc<Semaphore>,
		interval: Duration,
		queue_size: Arc<AtomicUsize>,
		notify_idle: Arc<Notify>,
	) -> ! {
		loop {
			// First, wait for a task to execute
			let task = task_receiver
				.recv()
				.await
				.expect("Failed to read task queue. Did the channel close?");

			// Acquire a permit so we are allowed to *start* running a task
			let permit = semaphore
				.acquire()
				.await
				.expect("Failed to acquire semaphore permit. Did the semaphore close?");
			// And forget it again. We add it exactly n seconds after the task is *done*
			permit.forget();

			// Clone the semaphore Arc and queue size counter
			let semaphore = semaphore.clone();
			let counter = queue_size.clone();
			let idle_notifier = notify_idle.clone();

			tokio::spawn(async move {
				// Now, async run the task!
				task().await;

				// Sleep for 3 seconds, and then add a permit
				sleep(interval).await;
				semaphore.add_permits(1);

				if counter.fetch_sub(1, Ordering::Relaxed) == 1 {
					// This was the last task in the queue.
					// Notify we are done!
					idle_notifier.notify_one();
				}
			});
		}
	}

	/// Schedules the given task. \
	/// Returns the time until the task starts. \
	/// Assumes all tasks are instant.
	///
	/// # Example
	///
	/// ```rs
	/// let wait_time = rate_limiter
	/// 	.schedule_task(Box::new(move || {
	/// 		Box::pin(async move {
	/// 			// Run task...
	/// 		})
	/// 	}));
	/// ```
	pub async fn schedule_task(&self, task: Task) -> Duration {
		self.task_queue_sender
			.send(task)
			.await
			.expect("Failed to schedule task. Did the channel close?");

		// Add 1 to the queue counter
		let position_in_queue = self.queued_tasks.fetch_add(1, Ordering::Relaxed);

		let batches_ahead = position_in_queue / self.amount;

		let estimated_waiting_time = self.per_time * batches_ahead as u32;

		return estimated_waiting_time;
	}

	/// Waits until all tasks in the queue are executed
	pub async fn wait_until_idle(&self) {
		self.notify_idle.notified().await
	}
}

#[cfg(test)]
mod tests {
	use tokio::sync::Mutex;

	use super::*;

	#[tokio::test]
	async fn test_creation_parameters() {
		assert!(RateLimiter::new(0, Duration::from_secs(1)).is_err());
		assert!(RateLimiter::new(1, Duration::from_secs(1)).is_ok());
		assert!(RateLimiter::new(100, Duration::from_secs(1)).is_ok());
	}

	#[tokio::test]
	async fn test_limit_small_tasks() {
		let rate_limiter = RateLimiter::new(1, Duration::from_secs(1)).unwrap();
		let task_1_status = Arc::new(Mutex::new(0));
		let task_1_status_check = task_1_status.clone();
		let task_2_status = Arc::new(Mutex::new(0));
		let task_2_status_check = task_2_status.clone();
		let task_3_status = Arc::new(Mutex::new(0));
		let task_3_status_check = task_3_status.clone();

		tokio::time::pause();

		// Task 1
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_1_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(200)).await;

					*task_1_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 2
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_2_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(200)).await;

					*task_2_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 3
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_3_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(200)).await;

					*task_3_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// TASK TIMINGS
		// 1: 0.000 -> 0.200 + 1ms
		// 2: 1.200 -> 1.400 + 1ms
		// 3: 2.400 -> 2.600 + 1ms

		// Starting first task
		assert_eq!(
			*task_1_status_check.clone().lock().await,
			0,
			"task 1 should not have started yet"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			1,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(201)).await; // 0.0 -> 0.201

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			1,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(799)).await; // 0.201 -> 1.000

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		// Starting second task
		tokio::time::advance(Duration::from_millis(202)).await; // 1.000 -> 1.202

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(201)).await; // 1.202 -> 1.403

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(597)).await; // 1.403 -> 2.000

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		// Starting third task
		tokio::time::advance(Duration::from_millis(404)).await; // 2.000 -> 2.403

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			1,
			"task 3 should have started"
		);

		tokio::time::advance(Duration::from_millis(201)).await; // 2.403 -> 2.604

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			1,
			"task 3 should have started"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should be done now"
		);
	}

	#[tokio::test]
	async fn test_limit_big_tasks() {
		let rate_limiter = RateLimiter::new(1, Duration::from_secs(1)).unwrap();
		let task_1_status = Arc::new(Mutex::new(0));
		let task_1_status_check = task_1_status.clone();
		let task_2_status = Arc::new(Mutex::new(0));
		let task_2_status_check = task_2_status.clone();
		let task_3_status = Arc::new(Mutex::new(0));
		let task_3_status_check = task_3_status.clone();

		tokio::time::pause();

		// Task 1
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_1_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(2000)).await;

					*task_1_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 2
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_2_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(2000)).await;

					*task_2_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 3
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_3_status.clone().lock_owned().await = 1;

					sleep(Duration::from_millis(2000)).await;

					*task_3_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// TASK TIMINGS
		// 1: 0.000 -> 2.000 + 1ms
		// 2: 3.000 -> 5.000 + 1ms
		// 3: 6.000 -> 8.000 + 1ms

		// Starting first task
		assert_eq!(
			*task_1_status_check.clone().lock().await,
			0,
			"task 1 should not have started yet"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			1,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(2001)).await; // 0.0 -> 2.0

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			1,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		// Starting second task
		tokio::time::advance(Duration::from_millis(1001)).await; // 2.0 -> 3.0

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(2001)).await; // 3.0 -> 5.0

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		// Starting third task
		tokio::time::advance(Duration::from_millis(1001)).await; // 5.0 -> 6.0

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			1,
			"task 3 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(2001)).await; // 6.0 -> 8.0

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should have started"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			1,
			"task 3 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be done now"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should be done now"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
	}

	#[tokio::test]
	async fn test_limit_variable_tasks() {
		let rate_limiter = RateLimiter::new(3, Duration::from_secs(3)).unwrap();
		let task_1_status = Arc::new(Mutex::new(0));
		let task_1_status_check = task_1_status.clone();
		let task_2_status = Arc::new(Mutex::new(0));
		let task_2_status_check = task_2_status.clone();
		let task_3_status = Arc::new(Mutex::new(0));
		let task_3_status_check = task_3_status.clone();
		let task_4_status = Arc::new(Mutex::new(0));
		let task_4_status_check = task_4_status.clone();
		let task_5_status = Arc::new(Mutex::new(0));
		let task_5_status_check = task_5_status.clone();
		let task_6_status = Arc::new(Mutex::new(0));
		let task_6_status_check = task_6_status.clone();

		tokio::time::pause();

		// Task 1
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_1_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(100)).await;
					*task_1_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 2
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_2_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(4000)).await;
					*task_2_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 3
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_3_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(100)).await;
					*task_3_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 4
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_4_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(100)).await;
					*task_4_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 5
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_5_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(100)).await;
					*task_5_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// Task 6
		rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					*task_6_status.clone().lock_owned().await = 1;
					sleep(Duration::from_millis(100)).await;
					*task_6_status.clone().lock_owned().await = 2;
				})
			}))
			.await;

		// TASK TIMINGS
		// 1: 0.000 -> 0.200
		// 2: 0.000 -> 4.000 -> Is part of the second group of 3 tasks per 3 seconds
		// 3: 0.000 -> 0.200
		//
		// 4: 3.200 -> 3.400
		// 5: 3.200 -> 3.400
		//
		// 6: 7.000 -> 7.200

		// Starting 1, 2 and 3
		assert_eq!(
			*task_1_status_check.clone().lock().await,
			0,
			"task 1 should not have started yet"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			0,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			0,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			0,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			0,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			1,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			1,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			0,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			0,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(200)).await; // 0.0 -> 0.2
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			0,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			0,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(3001)).await; // 0.2 -> 3.2
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			1,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			1,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(200)).await; // 3.2 -> 3.4
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			1,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			2,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			2,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(600)).await; // 3.4 -> 4.0
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			2,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			2,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			0,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(3000)).await; // 4.0 -> 7.0
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			2,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			2,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			1,
			"task 6 should not have started yet"
		);

		tokio::time::advance(Duration::from_millis(200)).await; // 7.0 -> 7.2
		tokio::task::yield_now().await;

		assert_eq!(
			*task_1_status_check.clone().lock().await,
			2,
			"task 1 should be running"
		);
		assert_eq!(
			*task_2_status_check.clone().lock().await,
			2,
			"task 2 should not have started yet"
		);
		assert_eq!(
			*task_3_status_check.clone().lock().await,
			2,
			"task 3 should not have started yet"
		);
		assert_eq!(
			*task_4_status_check.clone().lock().await,
			2,
			"task 4 should not have started yet"
		);
		assert_eq!(
			*task_5_status_check.clone().lock().await,
			2,
			"task 5 should not have started yet"
		);
		assert_eq!(
			*task_6_status_check.clone().lock().await,
			2,
			"task 6 should not have started yet"
		);
	}

	#[tokio::test]
	async fn test_eta() {
		let rate_limiter = RateLimiter::new(3, Duration::from_secs(3)).unwrap();

		tokio::time::pause();

		let eta_1 = rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					sleep(Duration::from_millis(10)).await;
				})
			}))
			.await;

		assert_eq!(eta_1, Duration::from_millis(0));

		let eta_2 = rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					sleep(Duration::from_millis(10)).await;
				})
			}))
			.await;

		assert_eq!(eta_2, Duration::from_millis(0));

		let eta_3 = rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					sleep(Duration::from_millis(10)).await;
				})
			}))
			.await;

		assert_eq!(eta_3, Duration::from_millis(0));

		let eta_4 = rate_limiter
			.schedule_task(Box::new(|| {
				Box::pin(async move {
					sleep(Duration::from_millis(10)).await;
				})
			}))
			.await;

		assert_eq!(eta_4, Duration::from_secs(3));
	}
}
