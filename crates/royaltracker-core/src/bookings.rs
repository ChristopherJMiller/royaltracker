use chrono::NaiveDate;
use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_storage::PriceRepo;
use royaltracker_types::{Booking, Brand, User};
use std::collections::HashMap;
use tracing::{info, warn};

/// Discover all bookings across both RCG brands and persist them.
///
/// Thin wrapper around [`discover_with_clients`] for callers that don't need
/// to reuse the authenticated clients (e.g. the web `register`/`refresh`
/// handlers).
pub async fn discover_and_persist_bookings(
    rcg_username: &str,
    rcg_password: &str,
    basic_auth_b64: &str,
    repo: &(dyn PriceRepo),
    user: &User,
) -> anyhow::Result<DiscoveryReport> {
    let (report, _clients) =
        discover_with_clients(rcg_username, rcg_password, basic_auth_b64, repo, user).await?;
    Ok(report)
}

/// Discover all bookings across both RCG brands and persist them, returning
/// the per-brand authenticated clients so the caller can reuse them for
/// downstream API calls without logging in again.
///
/// We deliberately do TWO logins (one per brand's auth host) because Phase 0
/// only verified that a JWT issued via the *Celebrity* host works for both
/// `?brand=R` and `?brand=C` queries. We never confirmed the reverse, so the
/// safe pattern is to log in to each brand's host and use that JWT for its
/// own brand's bookings query.
///
/// Cost: 2 logins + 2 bookings calls per invocation (~4 RCG requests).
pub async fn discover_with_clients(
    rcg_username: &str,
    rcg_password: &str,
    basic_auth_b64: &str,
    repo: &(dyn PriceRepo),
    user: &User,
) -> anyhow::Result<(DiscoveryReport, HashMap<Brand, CruiseClient>)> {
    let mut report = DiscoveryReport::default();
    let mut clients: HashMap<Brand, CruiseClient> = HashMap::new();

    for brand in [Brand::Royal, Brand::Celebrity] {
        let cfg = CruiseClientConfig::web(
            brand,
            rcg_username.to_string(),
            rcg_password.to_string(),
            basic_auth_b64.to_string(),
        );
        let client = match CruiseClient::new(cfg) {
            Ok(c) => c,
            Err(e) => {
                warn!(brand = ?brand, error = %e, "client init failed");
                report.errors.push(format!("{brand}: client init: {e}"));
                continue;
            }
        };

        let token = match client.login().await {
            Ok(t) => t,
            Err(e) => {
                warn!(brand = ?brand, error = %e, "login failed");
                report.errors.push(format!("{brand}: login: {e}"));
                continue;
            }
        };
        report.logins_ok += 1;

        let summaries = match client.list_bookings_for(brand).await {
            Ok(s) => s,
            Err(e) => {
                warn!(brand = ?brand, error = %e, "list_bookings_for failed");
                report.errors.push(format!("{brand}: bookings: {e}"));
                continue;
            }
        };

        for summary in summaries {
            let Some(reservation_id) = summary.reservation_id else {
                continue;
            };
            let ship_code = summary.ship_code.unwrap_or_default();
            let passenger_id = summary.primary_passenger_id;
            let sail_date = summary
                .sail_date
                .as_ref()
                .and_then(|s| {
                    NaiveDate::parse_from_str(s, "%Y%m%d")
                        .or_else(|_| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                        .ok()
                })
                .unwrap_or_else(|| chrono::Utc::now().date_naive());

            let booking = Booking {
                reservation_id: reservation_id.clone(),
                brand,
                account_id: token.account_id.clone(),
                ship_code,
                sail_date,
                passenger_id,
                nights: summary.number_of_nights,
                package_code: summary.package_code,
            };

            if let Err(e) = repo.upsert_booking(&booking).await {
                warn!(error = %e, "upsert_booking failed");
                report.errors.push(format!("upsert: {e}"));
                continue;
            }
            // Subscribe this user to the booking. Multiple users on the same
            // reservation (e.g. partners on the same cruise) each get their own
            // subscription instead of clobbering each other.
            if let Err(e) = repo
                .subscribe_user_to_booking(&reservation_id, user.id)
                .await
            {
                warn!(error = %e, "subscribe_user_to_booking failed");
                report.errors.push(format!("subscribe: {e}"));
                continue;
            }
            report.persisted += 1;
        }

        // Stash the authenticated client so the caller can reuse the JWT for
        // downstream calls (price fetches, etc.) without logging in again.
        clients.insert(brand, client);
    }

    info!(
        user = %user.rcg_username,
        persisted = report.persisted,
        logins_ok = report.logins_ok,
        errors = report.errors.len(),
        "bookings discovery complete"
    );

    Ok((report, clients))
}

#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub persisted: usize,
    pub logins_ok: usize,
    pub errors: Vec<String>,
}
