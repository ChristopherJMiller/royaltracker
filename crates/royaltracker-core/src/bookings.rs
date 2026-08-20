use chrono::NaiveDate;
use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_storage::PriceRepo;
use royaltracker_notify::{NotifyTarget, Notifier, TextAlert};
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
    // No bot here: web-initiated discovery (register/refresh) means the user is
    // already looking at the UI, so cabin-change pushes would be redundant.
    let (report, _clients) =
        discover_with_clients(rcg_username, rcg_password, basic_auth_b64, repo, user, None).await?;
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
    // When `Some`, cabin-assignment changes detected during this pass are pushed
    // to each reservation's subscribers. The scraper supplies it; web discovery
    // passes `None`.
    notifier: Option<&dyn Notifier>,
) -> anyhow::Result<(DiscoveryReport, HashMap<Brand, CruiseClient>)> {
    let mut report = DiscoveryReport::default();
    let mut clients: HashMap<Brand, CruiseClient> = HashMap::new();

    // Snapshot of what we already had persisted, taken before this pass upserts
    // anything, so we can detect cabin assignments/reassignments.
    let existing_bookings: HashMap<String, Booking> = repo
        .list_bookings()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|b| (b.reservation_id.clone(), b))
        .collect();

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
            let Some(reservation_id) = summary.reservation_id.clone() else {
                continue;
            };
            let displayed_stateroom = summary.stateroom();
            let is_gty = summary.is_gty();
            let own_pax_ids = summary.own_passenger_ids();
            // Wire-format sail date (YYYYMMDD) as the API wants it for order history.
            let sail_wire = summary.sail_date.clone();
            let ship_code = summary.ship_code.clone().unwrap_or_default();
            let passenger_id = summary.primary_passenger_id.clone();
            let sail_date = summary
                .sail_date
                .as_ref()
                .and_then(|s| {
                    NaiveDate::parse_from_str(s, "%Y%m%d")
                        .or_else(|_| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                        .ok()
                })
                .unwrap_or_else(|| chrono::Utc::now().date_naive());

            // For guarantee cabins, recover the real assigned room from the
            // reservation's purchased-order records (the room leaks there even
            // while the booking still displays "GTY"). Best-effort: a failure
            // just leaves the cabin undiscovered this cycle.
            let assigned_stateroom = match (is_gty, passenger_id.as_deref(), sail_wire.as_deref()) {
                (true, Some(pax), Some(sail)) if !own_pax_ids.is_empty() => {
                    match client
                        .discover_assigned_stateroom(
                            brand,
                            &ship_code,
                            sail,
                            &reservation_id,
                            pax,
                            &own_pax_ids,
                        )
                        .await
                    {
                        Ok(room) => {
                            if let Some(r) = &room {
                                info!(reservation = %reservation_id, cabin = %r, "recovered GTY cabin");
                            }
                            room
                        }
                        Err(e) => {
                            warn!(reservation = %reservation_id, error = %e, "assigned-stateroom discovery failed");
                            None
                        }
                    }
                }
                _ => None,
            };

            let booking = Booking {
                reservation_id: reservation_id.clone(),
                brand,
                account_id: token.account_id.clone(),
                ship_code,
                sail_date,
                passenger_id,
                nights: summary.number_of_nights,
                package_code: summary.package_code,
                stateroom: displayed_stateroom,
                assigned_stateroom,
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

            // Push a cabin-assignment alert when we have a notifier and this
            // booking existed before this pass (skips first-time discovery/backfill).
            if let Some(notifier) = notifier {
                if let Some(old) = existing_bookings.get(&reservation_id) {
                    if let Some((headline, body, deck_cabin)) = detect_cabin_event(old, &booking) {
                        let deck = royaltracker_types::deck_of_cabin(&deck_cabin)
                            .map(|d| format!(" · deck {d}"))
                            .unwrap_or_default();
                        let ship = royaltracker_types::ship_name(&booking.ship_code)
                            .unwrap_or(booking.ship_code.as_str());
                        let text = format!(
                            "{headline}\n{ship} · {}\n{body}{deck}",
                            booking.sail_date.format("%b %-d, %Y")
                        );
                        let alert = TextAlert {
                            title: &headline,
                            body: &text,
                            manage_url: None,
                        };
                        match repo.list_subscribers_for_reservation(&reservation_id).await {
                            Ok(subs) => {
                                for s in subs {
                                    let target = NotifyTarget::Telegram {
                                        chat_id: s.telegram_chat_id,
                                    };
                                    if let Err(e) = notifier.notify_text(&target, &alert).await {
                                        warn!(error = %e, chat_id = s.telegram_chat_id, "cabin-change alert failed");
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "subscriber lookup for cabin alert failed")
                            }
                        }
                    }
                }
            }
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

/// Decide whether a booking's cabin changed in a way worth alerting on,
/// comparing the previously-stored booking with the freshly-discovered one.
/// Returns `(headline, body, cabin_for_deck)`.
///
/// Intentionally keys the "assigned" signal off the *displayed* stateroom
/// flipping from `"GTY"` to a real number (RCG officially assigning the
/// guarantee) rather than `assigned_stateroom` going `None → Some` — the latter
/// fires on the one-time backfill of already-known rooms, the former never does.
fn detect_cabin_event(old: &Booking, new: &Booking) -> Option<(&'static str, String, String)> {
    // Official assignment: displayed cabin went from the "GTY" placeholder to a
    // real room.
    if old.stateroom.as_deref() == Some("GTY") {
        if let Some(room) = new.stateroom.as_deref() {
            if room != "GTY" && !room.is_empty() {
                return Some((
                    "🎉 Guarantee cabin assigned!",
                    format!("Your room: {room}"),
                    room.to_string(),
                ));
            }
        }
    }
    // Reassignment: the recovered/assigned room changed between two known values.
    if let (Some(a), Some(b)) = (
        old.assigned_stateroom.as_deref(),
        new.assigned_stateroom.as_deref(),
    ) {
        if a != b {
            return Some(("🔀 Assigned cabin changed", format!("{a} → {b}"), b.to_string()));
        }
    }
    None
}

#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub persisted: usize,
    pub logins_ok: usize,
    pub errors: Vec<String>,
}
