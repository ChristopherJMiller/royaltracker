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

/// Sleep a uniformly-random duration in `[lo, hi]`. Used to spread the public
/// sweep across its window without a metronomic cadence.
pub async fn jittered_delay(lo: Duration, hi: Duration) {
    if hi <= lo {
        tokio::time::sleep(lo).await;
        return;
    }
    let span = (hi - lo).as_millis() as u64;
    let extra = rand::thread_rng().gen_range(0..=span);
    tokio::time::sleep(lo + Duration::from_millis(extra)).await;
}
