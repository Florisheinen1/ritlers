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
	sync::{Semaphore, mpsc},
	time::sleep,
};

use crate::TaskResult;

type Task = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = TaskResult> + Send>> + Send + Sync>;

/// Task-length aware rate limiter
///
/// # Example:
/// ```rs
/// // Limit 3 tasks per 5 seconds
/// let ritlers = RateLimiter::new(3, Duration::from_secs(5));
/// let schedule_wait_time = ritlers
/// 	.schedule_task(async {
/// 		// Run task...
/// 	}).await;
/// ```
pub struct RateLimiter {
	task_queue_sender: mpsc::Sender<Task>,
	queued_tasks: Arc<AtomicUsize>,
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

		let (sender, receiver) = mpsc::channel::<Task>(100);

		// Keep track of the size of the queue for ETA calculations
		let queue_size = Arc::new(AtomicUsize::new(0));
		let queue_size_upper = queue_size.clone();

		let sender_for_retry = sender.clone();
		tokio::spawn(async move {
			Self::run_tasks(
				receiver,
				sender_for_retry,
				semaphore,
				per_time,
				queue_size_upper,
			)
			.await;
		});

		Ok(Self {
			task_queue_sender: sender,
			queued_tasks: queue_size,
			amount,
			per_time,
		})
	}

	/// Runs all scheduled tasks.
	/// Thread blocking and runs indefinitely
	async fn run_tasks(
		mut task_receiver: mpsc::Receiver<Task>,
		task_sender: mpsc::Sender<Task>,
		semaphore: Arc<Semaphore>,
		interval: Duration,
		queue_size: Arc<AtomicUsize>,
	) -> ! {
		loop {
			let task = task_receiver
				.recv()
				.await
				.expect("Failed to read task queue. Did the channel close?");

			let permit = semaphore
				.acquire()
				.await
				.expect("Failed to acquire semaphore permit. Did the semaphore close?");

			// New permit will be added again after the task is finished
			permit.forget();

			let semaphore = semaphore.clone();
			let counter = queue_size.clone();
			let sender = task_sender.clone();

			// Run task itself on separate thread so we can immediately
			// wait for new tasks
			tokio::spawn(async move {
				let result = task().await;

				// Sleep for `interval` time before adding a permit to ensure the
				// rate limit is adhered to
				sleep(interval).await;
				semaphore.add_permits(1);

				match result {
					TaskResult::Success => {
						counter.fetch_sub(1, Ordering::Relaxed);
					}
					// Re-enqueue the task at the back of the queue so it runs
					// again as soon as a slot is available
					TaskResult::TryAgain => {
						let _ = sender.send(task).await;
					}
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
	/// 	.schedule_task(async {
	/// 		// Run task...
	/// 	});
	/// ```
	pub async fn schedule_task<Fut>(&self, fut: Fut) -> Duration
	where
		Fut: Future<Output = ()> + Send + 'static,
	{
		// Wrap the one-shot future so it fits the retryable Task type.
		// The Mutex<Option<_>> lets us move the future into an Arc<Fn()> without
		// requiring Fn to own it multiple times; take() is called exactly once
		// because this task always returns Success.
		let fut = std::sync::Mutex::new(Some(fut));
		let task: Task = Arc::new(move || {
			let f = fut.lock().unwrap().take().expect("task already consumed");
			Box::pin(async move {
				f.await;
				TaskResult::Success
			}) as Pin<Box<dyn Future<Output = TaskResult> + Send>>
		});

		self.task_queue_sender
			.send(task)
			.await
			.expect("Failed to schedule task. Did the channel close?");

		let position_in_queue = self.queued_tasks.fetch_add(1, Ordering::Relaxed);

		let batches_ahead = position_in_queue / self.amount;

		self.per_time * batches_ahead as u32
	}

	/// Schedules a retryable task using a factory closure. \
	/// The factory is called each time the task runs. If it returns
	/// [`TaskResult::TryAgain`], the task is re-queued and will run again
	/// as soon as the next rate-limit slot is available. \
	/// Returns the estimated time until the first attempt starts.
	///
	/// # Example
	///
	/// ```rs
	/// rate_limiter.schedule_task_with_retry(|| async {
	/// 	match api_call().await {
	/// 		Ok(_) => TaskResult::Success,
	/// 		Err(ApiError::RateLimit) => TaskResult::TryAgain,
	/// 	}
	/// }).await;
	/// ```
	pub async fn schedule_task_with_retry<F, Fut>(&self, factory: F) -> Duration
	where
		F: Fn() -> Fut + Send + Sync + 'static,
		Fut: Future<Output = TaskResult> + Send + 'static,
	{
		let factory = Arc::new(factory);
		let task: Task = Arc::new(move || {
			let factory = factory.clone();
			Box::pin(async move { factory().await })
				as Pin<Box<dyn Future<Output = TaskResult> + Send>>
		});

		self.task_queue_sender
			.send(task)
			.await
			.expect("Failed to schedule task. Did the channel close?");

		let position_in_queue = self.queued_tasks.fetch_add(1, Ordering::Relaxed);

		let batches_ahead = position_in_queue / self.amount;

		self.per_time * batches_ahead as u32
	}
}

#[cfg(test)]
mod tests {
	use tokio::sync::Mutex;

	use super::*;

	#[tokio::test]
	async fn test_creation_parameters() {
		assert!(
			RateLimiter::new(0, Duration::from_secs(1)).is_err(),
			"Zero tasks allowed concurrently is not allowed"
		);
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
			.schedule_task(async move {
				*task_1_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(200)).await;

				*task_1_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 2
		rate_limiter
			.schedule_task(async move {
				*task_2_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(200)).await;

				*task_2_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 3
		rate_limiter
			.schedule_task(async move {
				*task_3_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(200)).await;

				*task_3_status.clone().lock_owned().await = 2;
			})
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
			.schedule_task(async move {
				*task_1_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(2000)).await;

				*task_1_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 2
		rate_limiter
			.schedule_task(async move {
				*task_2_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(2000)).await;

				*task_2_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 3
		rate_limiter
			.schedule_task(async move {
				*task_3_status.clone().lock_owned().await = 1;

				sleep(Duration::from_millis(2000)).await;

				*task_3_status.clone().lock_owned().await = 2;
			})
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
			.schedule_task(async move {
				*task_1_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(100)).await;
				*task_1_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 2
		rate_limiter
			.schedule_task(async move {
				*task_2_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(4000)).await;
				*task_2_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 3
		rate_limiter
			.schedule_task(async move {
				*task_3_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(100)).await;
				*task_3_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 4
		rate_limiter
			.schedule_task(async move {
				*task_4_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(100)).await;
				*task_4_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 5
		rate_limiter
			.schedule_task(async move {
				*task_5_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(100)).await;
				*task_5_status.clone().lock_owned().await = 2;
			})
			.await;

		// Task 6
		rate_limiter
			.schedule_task(async move {
				*task_6_status.clone().lock_owned().await = 1;
				sleep(Duration::from_millis(100)).await;
				*task_6_status.clone().lock_owned().await = 2;
			})
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
			.schedule_task(async {
				sleep(Duration::from_millis(10)).await;
			})
			.await;

		assert_eq!(
			eta_1,
			Duration::from_millis(0),
			"First task should start immediately"
		);

		let eta_2 = rate_limiter
			.schedule_task(async {
				sleep(Duration::from_millis(10)).await;
			})
			.await;

		assert_eq!(
			eta_2,
			Duration::from_millis(0),
			"Second task should start immediately"
		);

		let eta_3 = rate_limiter
			.schedule_task(async {
				sleep(Duration::from_millis(10)).await;
			})
			.await;

		assert_eq!(
			eta_3,
			Duration::from_millis(0),
			"Third task should start immediately"
		);

		let eta_4 = rate_limiter
			.schedule_task(async {
				sleep(Duration::from_millis(10)).await;
			})
			.await;

		assert_eq!(
			eta_4,
			Duration::from_secs(3),
			"Fourth task should start after at least 3 seconds"
		);
	}

	#[tokio::test]
	async fn test_retry() {
		let rate_limiter = RateLimiter::new(1, Duration::from_millis(50)).unwrap();
		let attempt_count = Arc::new(AtomicUsize::new(0));
		let attempt_count_check = attempt_count.clone();
		let succeeded = Arc::new(Mutex::new(false));
		let succeeded_check = succeeded.clone();

		rate_limiter
			.schedule_task_with_retry(move || {
				let count = attempt_count.clone();
				let succeeded = succeeded.clone();
				async move {
					let n = count.fetch_add(1, Ordering::Relaxed) + 1;
					if n < 3 {
						TaskResult::TryAgain
					} else {
						*succeeded.lock().await = true;
						TaskResult::Success
					}
				}
			})
			.await;

		// Allow all retry attempts to complete (3 attempts * 50ms interval)
		tokio::time::sleep(Duration::from_millis(300)).await;

		assert_eq!(
			attempt_count_check.load(Ordering::Relaxed),
			3,
			"Should have attempted 3 times (2 retries)"
		);
		assert!(*succeeded_check.lock().await, "Task should have succeeded");
	}
}
