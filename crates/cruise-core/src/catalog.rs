use cruise_api::CruiseClient;
use cruise_storage::{CatalogEntry, PriceRepo};
use cruise_types::Booking;
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
