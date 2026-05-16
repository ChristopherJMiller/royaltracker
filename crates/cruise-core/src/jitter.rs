use rand::Rng;
use std::time::Duration;

/// Sleep for a uniformly-distributed time in [0, max_minutes] minutes.
/// Use at the start of the scraper so we don't fire on the cron's exact second.
pub async fn sleep_with_jitter(max_minutes: u32) {
    if max_minutes == 0 {
        return;
    }
    let secs = rand::thread_rng().gen_range(0..(max_minutes as u64 * 60));
    tracing::info!(jitter_seconds = secs, "jitter sleep");
    tokio::time::sleep(Duration::from_secs(secs)).await;
}
