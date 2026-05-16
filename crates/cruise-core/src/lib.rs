pub mod bookings;
pub mod catalog;
pub mod diff;
pub mod jitter;
pub mod scrape;

pub use bookings::discover_and_persist_bookings;
pub use catalog::refresh_catalog_for_booking;
pub use diff::detect_diff;
pub use jitter::sleep_with_jitter;
pub use scrape::{run_scrape_cycle, ScrapeContext, ScrapeOutcome};
