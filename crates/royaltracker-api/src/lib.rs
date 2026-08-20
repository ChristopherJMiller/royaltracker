mod auth;
mod catalog;
mod client;
mod error;
mod graphql;
mod public;

pub use auth::{TokenState, decode_account_id};
pub use catalog::ProductPrice;
pub use client::{
    build_emulated_client, warm_up_host, BookingSummary, CruiseClient, CruiseClientConfig,
    DEFAULT_USER_AGENT,
};
pub use error::ApiError;
pub use graphql::{parse_money_cents, Category, GraphqlProduct};
pub use public::{
    is_bot_challenge, PacingConfig, PublicClient, PublicClientConfig, PublicFetch, PublicProduct,
};

pub const WEB_APP_KEY: &str = "hyNNqIPHHzaLzVpcICPdAdbFV8yvTsAm";
pub const MOBILE_APP_KEY: &str = "cdCNc04srNq4rBvKofw1aC50dsdSaPuc";
