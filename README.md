# RITLERS

A Round-Trip aware rate limiter, made in Rust.
Useful for calling APIs that apply strict rate limiting.

## Difference from other rate limiters

Most other rate limiters limit the start of each provided task. Unfortunately, due to network routing inconsistencies, even when requests are periodically sent conforming the rate limit, all requests might still end up at the endpoint simultaneously. Ritlers therefore waits until the provided tasks are "done" (E.g. received a response) before rate limit time continues.
