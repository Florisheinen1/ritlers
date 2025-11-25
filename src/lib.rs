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

type Task = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

pub struct RateLimiter {
	task_queue_sender: mpsc::Sender<Task>,
	queued_tasks: Arc<AtomicUsize>,

	amount: usize,
	per_time: Duration,
}

impl RateLimiter {
	pub fn new(amount: usize, per_time: Duration) -> Self {
		// The semaphore indicates the maximum amount of concurrent tasks running
		let semaphore = Arc::new(Semaphore::new(amount));

		// Create the send and receive end of the Tasks channel
		let (sender, receiver) = mpsc::channel::<Task>(100);

		// Keep track of the size of the queue for ETA calculations
		let queue_size = Arc::new(AtomicUsize::new(0));
		let queue_size_upper = queue_size.clone();

		// Start running tasks
		tokio::spawn(async move {
			Self::run_tasks(receiver, semaphore, per_time, queue_size_upper).await;
		});

		Self {
			task_queue_sender: sender,
			queued_tasks: queue_size,

			amount,
			per_time,
		}
	}

	/// Runs all tasks queued
	async fn run_tasks(
		mut task_receiver: mpsc::Receiver<Task>,
		semaphore: Arc<Semaphore>,
		interval: Duration,
		queue_size: Arc<AtomicUsize>,
	) {
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

			tokio::spawn(async move {
				// Now, async run the task!
				task().await;

				// Sleep for 3 seconds, and then add a permit
				sleep(interval).await;
				semaphore.add_permits(1);

				counter.fetch_sub(1, Ordering::Relaxed);
			});
		}
	}

	/// Schedules the given task under the current rate limit
	/// Returns the estimated waiting time
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
}
