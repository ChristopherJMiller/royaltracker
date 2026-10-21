use crate::princess::{PrincessExtrasCache, PrincessPriceCache};
use royaltracker_api::princess::PrincessGuestClient;
use royaltracker_api::CruiseClient;
use royaltracker_storage::{CatalogEntry, PriceRepo};
use royaltracker_types::Booking;
use tracing::{info, warn};

/// Pull the anonymous GraphQL catalog for this booking and upsert into catalog_cache.
/// Returns count of products persisted.
pub async fn refresh_catalog_for_booking(
    api: &CruiseClient,
    repo: &(dyn PriceRepo),
    booking: &Booking,
) -> anyhow::Result<usize> {
    let pairs = api
        .fetch_catalog(
            &booking.ship_code,
            booking.sail_date,
            booking.passenger_id.as_deref(),
            Some(&booking.reservation_id),
        )
        .await?;

    let mut persisted = 0;
    for (cat, prod) in pairs {
        let price = prod.first_price();
        let entry = CatalogEntry {
            reservation_id: booking.reservation_id.clone(),
            category_id: cat.id,
            category_name: cat.name,
            product_code: prod.id.clone(),
            title: prod.title.clone().unwrap_or_default(),
            summary: None,
            starting_price: price.and_then(|p| p.promo_price_f64()),
            currency: price.and_then(|p| p.currency.clone()),
            price_label: price.and_then(|p| p.best_price_label()),
            base_price_label: price.and_then(|p| p.best_base_label()),
            unit_label: price
                .and_then(|p| p.sales_unit.as_ref())
                .and_then(|u| u.label.clone().or_else(|| u.name.clone())),
        };
        match repo.upsert_catalog_entry(&entry).await {
            Ok(()) => persisted += 1,
            Err(e) => warn!(error = %e, "upsert_catalog_entry failed"),
        }
    }
    info!(reservation = %booking.reservation_id, persisted, "catalog refresh complete");
    Ok(persisted)
}

/// Refresh the Princess catalog (excursions for the booking's voyage + extras)
/// via the guest SFCC client, with extras from the shared TTL'd cache.
/// Returns the count of products persisted.
pub async fn refresh_princess_catalog(
    client: &PrincessGuestClient,
    extras: &PrincessExtrasCache,
    repo: &(dyn PriceRepo),
    booking: &Booking,
) -> anyhow::Result<usize> {
    let voyage = booking
        .voyage_code
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Princess booking has no voyage code"))?;
    let currency = booking.currency.as_deref().unwrap_or("USD");
    let mut cache = PrincessPriceCache::default();
    let products = cache.products_shared(client, extras, voyage, currency).await?;

    let mut persisted = 0;
    for (_, p) in products {
        let entry = CatalogEntry {
            reservation_id: booking.reservation_id.clone(),
            category_id: p.category_id.clone(),
            category_name: p.category_name.clone(),
            product_code: p.product_code.clone(),
            title: p.title.clone().unwrap_or_default(),
            summary: None,
            starting_price: p.promo_dollars(),
            currency: p.currency.clone(),
            price_label: p.promo_label.clone(),
            base_price_label: p.base_label.clone(),
            unit_label: p.unit_label.clone(),
        };
        match repo.upsert_catalog_entry(&entry).await {
            Ok(()) => persisted += 1,
            Err(e) => warn!(error = %e, "upsert_catalog_entry failed"),
        }
    }
    info!(reservation = %booking.reservation_id, persisted, "princess catalog refresh complete");
    Ok(persisted)
}
