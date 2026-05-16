mod auth;
mod catalog;
mod client;
mod error;
mod graphql;

pub use auth::{TokenState, decode_account_id};
pub use catalog::ProductPrice;
pub use client::{BookingSummary, CruiseClient, CruiseClientConfig};
pub use error::ApiError;
pub use graphql::{Category, GraphqlProduct};

pub const WEB_APP_KEY: &str = "hyNNqIPHHzaLzVpcICPdAdbFV8yvTsAm";
pub const MOBILE_APP_KEY: &str = "cdCNc04srNq4rBvKofw1aC50dsdSaPuc";
