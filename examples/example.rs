use std::time::Duration;

use reqwest::Client;
use ritlers::RateLimiter;
use serde::Deserialize;
use tokio::{
	self,
	sync::oneshot::{self, Receiver, error::TryRecvError},
};

// API endpoint to get randm dog photos
const DOG_API_ENDPOINT: &'static str = "https://dog.ceo/api/breeds/image/random";

#[derive(Debug, Deserialize)]
pub struct ApiResponse {
	pub message: String,
	pub status: String,
}
#[tokio::main]
async fn main() -> Result<(), ()> {
	// Only 3 links per 5 seconds
	let ritlers = RateLimiter::new(3, Duration::from_secs(5));

	let api_client = reqwest::ClientBuilder::new()
		.build()
		.expect("Failed to build API client");

	let mut receivers: Vec<Receiver<String>> = vec![];

	// Schedule 10 fetch tasks
	for x in 0..10 {
		let client_clone = api_client.clone();
		let (tx, rx) = oneshot::channel();
		receivers.push(rx);
		println!("Adding task {x}");
		ritlers
			.schedule_task(Box::new(move || {
				Box::pin(async move {
					println!("Starting to fetch task {x}");
					let random_dog_url = fetch_random_dog_link(&client_clone).await;
					println!("Fetched task {x}");
					tx.send(random_dog_url).expect("Failed to send dog url");
				})
			}))
			.await;
	}

	// Wait for the 10 tasks to be done, independent of order
	while !receivers.is_empty() {
		receivers.retain_mut(|rx| match rx.try_recv() {
			Ok(url) => {
				println!("Received url: {url}");
				false
			}
			Err(TryRecvError::Empty) => true,
			Err(TryRecvError::Closed) => unreachable!("Should not have a closed channel"),
		})
	}

	Ok(())
}

/// Fetches a random URL to a dog photo
async fn fetch_random_dog_link(client: &Client) -> String {
	let response = client
		.get(DOG_API_ENDPOINT)
		.send()
		.await
		.expect("Failed to send request");

	let response_text = response
		.text()
		.await
		.expect("Failed to get response text body");

	let response: ApiResponse =
		serde_json::from_str(&response_text).expect("Failed to deserialize API response");

	response.message
}
