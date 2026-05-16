//! Anonymous GraphQL catalog. Public endpoint — uses only the `appkey` header,
//! no JWT. Returns "starting from" prices (NOT personalized). Use for enumerating
//! what products exist for a booking; the daily scraper fetches personalized
//! prices via the authenticated REST `catalog/v2` endpoint per watched product.

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;

pub const GRAPHQL_URL: &str = "https://aws-prd.api.rccl.com/en/royal/web/graphql";

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphqlProduct {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    /// The API returns this as an array (one entry per pricing tier/sales unit).
    /// We typically only care about the first entry. Prices come back as strings.
    #[serde(default)]
    pub price: Vec<GraphqlPrice>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlPrice {
    #[serde(default)]
    pub currency: Option<String>,
    /// Comes from the API as a string like `"140.99"`. Parse via `promo_price_f64()`.
    #[serde(default)]
    pub promotional_price: Option<String>,
    #[serde(default)]
    pub shipboard_price: Option<String>,
    #[serde(default)]
    pub formatted_promotional_price: Option<String>,
    #[serde(default)]
    pub formatted_base_price: Option<String>,
    #[serde(default)]
    pub formatted_daily_price: Option<String>,
    #[serde(default)]
    pub formatted_promo_daily_price: Option<String>,
    #[serde(default)]
    pub sales_unit: Option<GraphqlSalesUnit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlSalesUnit {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

impl GraphqlPrice {
    pub fn promo_price_f64(&self) -> Option<f64> {
        // Prefer per-day numeric if present; fall back to one-time promo price.
        // (Display label still uses what the API gave us; this is just for sorting.)
        self.promotional_price.as_ref().and_then(|s| s.parse().ok())
    }

    pub fn best_price_label(&self) -> Option<String> {
        self.formatted_promo_daily_price
            .clone()
            .or_else(|| self.formatted_promotional_price.clone())
    }

    pub fn best_base_label(&self) -> Option<String> {
        self.formatted_daily_price
            .clone()
            .or_else(|| self.formatted_base_price.clone())
    }
}

impl GraphqlProduct {
    pub fn first_price(&self) -> Option<&GraphqlPrice> {
        self.price.first()
    }
}

pub async fn fetch_categories(
    http: &wreq::Client,
    app_key: &str,
    user_agent: &str,
    ship_code: &str,
    sail_date: NaiveDate,
) -> Result<Vec<Category>, ApiError> {
    let body = json!({
        "operationName": "WebCategories",
        "variables": {
            "sailDate": sail_date.format("%Y-%m-%d").to_string(),
            "shipCode": ship_code,
        },
        "query": "query WebCategories($shipCode: ShipCodeScalar!, $sailDate: LocalDateScalar!, $regionCode: String) { categories(shipCode: $shipCode, sailDate: $sailDate, regionCode: $regionCode, filter: {limitCategoriesWithProducts: true}) { ... on CategoryResultSuccess { categories { id name } } } }",
    });

    let resp = http
        .post(GRAPHQL_URL)
        .header("User-Agent", user_agent)
        .header("Accept", "application/json")
        .header("appkey", app_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ApiError::Status {
            status: status.as_u16(),
            body,
        });
    }

    let value: serde_json::Value = resp.json().await?;
    let cats = value
        .get("data")
        .and_then(|d| d.get("categories"))
        .and_then(|c| c.get("categories"))
        .and_then(|arr| serde_json::from_value::<Vec<Category>>(arr.clone()).ok())
        .unwrap_or_default();
    Ok(cats)
}

pub async fn fetch_products_in_category(
    http: &wreq::Client,
    app_key: &str,
    user_agent: &str,
    ship_code: &str,
    sail_date: NaiveDate,
    category_id: &str,
    passenger_id: Option<&str>,
    reservation_id: Option<&str>,
    currency: &str,
) -> Result<Vec<GraphqlProduct>, ApiError> {
    let pax = passenger_id.unwrap_or("");
    let res = reservation_id.unwrap_or("000000");
    let query = "query WebProductsByCategory($category:String!,$passengerId:String,$shipCode:ShipCodeScalar!,$sailDate:LocalDateScalar!,$reservationId:String,$pageSize:Long,$currentPage:Long,$sorting:Sorting,$filter:FilterInput,$currencyCode:String!){products(category:$category,guestTypes:[ADULT],passengerId:$passengerId,shipCode:$shipCode,sailDate:$sailDate,reservationId:$reservationId,pageSize:$pageSize,currentPage:$currentPage,sorting:$sorting,filter:$filter,currencyIso:$currencyCode){... on CommerceProductResultSuccess{commerceProducts{id title variantOptions{code name} price{currency promotionalPrice shipboardPrice formattedPromotionalPrice formattedBasePrice formattedDailyPrice formattedPromoDailyPrice salesUnit{code name label}}}}}}";

    let mut all = Vec::new();
    for page in 0i64..50 {
        let body = json!({
            "operationName": "WebProductsByCategory",
            "variables": {
                "category": category_id,
                "passengerId": pax,
                "shipCode": ship_code,
                "sailDate": sail_date.format("%Y-%m-%d").to_string(),
                "reservationId": res,
                "pageSize": 12,
                "currentPage": page,
                "sorting": { "sortKey": "RANK", "sortKeyOrder": "ASCENDING" },
                "filter": { "includeVariantProducts": false },
                "currencyCode": currency,
                "includeFilterInfo": false,
                "includeIfABexperience": false,
            },
            "query": query,
        });

        let resp = http
            .post(GRAPHQL_URL)
            .header("User-Agent", user_agent)
            .header("Accept", "application/json")
            .header("appkey", app_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(status = ?status, page, body = %body, "products graphql non-success");
            break;
        }
        let text = resp.text().await?;
        if page == 0 {
            tracing::info!(category = category_id, body_len = text.len(), body = %text.chars().take(800).collect::<String>(), "products graphql page 0");
        }
        let value: serde_json::Value = serde_json::from_str(&text)?;
        let page_products = value
            .get("data")
            .and_then(|d| d.get("products"))
            .and_then(|p| p.get("commerceProducts"))
            .and_then(|arr| serde_json::from_value::<Vec<GraphqlProduct>>(arr.clone()).ok())
            .unwrap_or_default();
        if page_products.is_empty() {
            break;
        }
        all.extend(page_products);
    }
    Ok(all)
}
