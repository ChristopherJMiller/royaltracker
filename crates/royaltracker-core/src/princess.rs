//! Princess account discovery: one login, one booking context, persisted.

use crate::bookings::DiscoveryReport;
use royaltracker_api::princess::{PrincessAccountClient, PrincessBookingContext};
use royaltracker_storage::{PriceRepo, SailingKey};
use royaltracker_types::{Booking, Brand, CruiseAccount, User};
use tracing::warn;

pub const PRINCESS_NO_BOOKING_MSG: &str =
    "Signed in to Princess, but couldn't open a booking — multi-booking accounts aren't supported yet.";

/// Map a Princess booking context to a `Booking`. Only tracking fields are kept.
pub fn booking_from_context(ctx: &PrincessBookingContext, account_id: i64) -> Booking {
    Booking {
        reservation_id: ctx.booking_id.clone(),
        brand: Brand::Princess,
        account_id: format!("princess:{account_id}"),
        ship_code: ctx.ship_code.clone(),
        sail_date: ctx.sail_date,
        passenger_id: Some(ctx.pax_id.clone()),
        nights: ctx.nights,
        package_code: None,
        stateroom: None,
        assigned_stateroom: None,
        voyage_code: Some(ctx.voyage_code.clone()),
        currency: Some(ctx.currency.clone()),
    }
}

/// Log in to Princess once (no retry), persist the booking and subscribe the user.
pub async fn discover_princess(
    repo: &dyn PriceRepo,
    user: &User,
    account: &CruiseAccount,
    password: &str,
) -> anyhow::Result<DiscoveryReport> {
    // One client per call: its cookie jar must not be shared across accounts.
    let client = PrincessAccountClient::new()?;
    let ctx = client.login_and_context(&account.login, password).await?;
    persist_context(repo, user, account, ctx).await
}

pub(crate) async fn persist_context(
    repo: &dyn PriceRepo,
    user: &User,
    account: &CruiseAccount,
    ctx: Option<PrincessBookingContext>,
) -> anyhow::Result<DiscoveryReport> {
    let mut report = DiscoveryReport { logins_ok: 1, ..Default::default() };
    let Some(ctx) = ctx else {
        report.errors.push(PRINCESS_NO_BOOKING_MSG.to_string());
        return Ok(report);
    };
    let booking = booking_from_context(&ctx, account.id);
    let key = SailingKey {
        brand: Brand::Princess,
        ship_code: &ctx.ship_code,
        sail_date: ctx.sail_date,
        voyage_code: Some(&ctx.voyage_code),
    };
    if let Err(e) = repo.upsert_sailing(&key, Some(&ctx.currency)).await {
        warn!(error = %e, "princess upsert_sailing failed");
        report.errors.push(format!("sailing: {e}"));
        return Ok(report);
    }
    if let Err(e) = repo.upsert_booking(&booking).await {
        warn!(error = %e, "princess upsert_booking failed");
        report.errors.push(format!("upsert: {e}"));
        return Ok(report);
    }
    if let Err(e) = repo.subscribe_user_to_booking(&booking.reservation_id, user.id).await {
        warn!(error = %e, "princess subscribe_user_to_booking failed");
        report.errors.push(format!("subscribe: {e}"));
        return Ok(report);
    }
    report.persisted = 1;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookings::login_outcome;
    use chrono::NaiveDate;

    fn ctx() -> PrincessBookingContext {
        PrincessBookingContext {
            booking_id: "AB12CD".into(),
            ship_code: "SKY".into(),
            voyage_code: "X637".into(),
            pax_id: "P1".into(),
            nights: Some(7),
            sail_date: NaiveDate::from_ymd_opt(2027, 3, 1).unwrap(),
            currency: "USD".into(),
        }
    }

    #[test]
    fn booking_from_context_maps_fields() {
        let b = booking_from_context(&ctx(), 7);
        assert_eq!(b.brand, Brand::Princess);
        assert_eq!(b.reservation_id, "AB12CD");
        assert_eq!(b.account_id, "princess:7");
        assert_eq!(b.voyage_code.as_deref(), Some("X637"));
        assert_eq!(b.currency.as_deref(), Some("USD"));
        assert_eq!(b.passenger_id.as_deref(), Some("P1"));
        assert!(b.stateroom.is_none() && b.assigned_stateroom.is_none() && b.package_code.is_none());
    }

    #[test]
    fn princess_discovery_no_booking_records_error() {
        let r = DiscoveryReport { persisted: 0, logins_ok: 1, errors: vec![PRINCESS_NO_BOOKING_MSG.into()] };
        assert_eq!(login_outcome(&r).as_deref(), Some(PRINCESS_NO_BOOKING_MSG));
    }
}
