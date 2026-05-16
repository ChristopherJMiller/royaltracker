#[cfg(all(feature = "sqlite", feature = "postgres"))]
compile_error!("royaltracker-storage: enable exactly one of: sqlite, postgres");

#[cfg(not(any(feature = "sqlite", feature = "postgres")))]
compile_error!("royaltracker-storage: enable one of: sqlite, postgres");

mod repo;

#[cfg(feature = "sqlite")]
mod sqlite;
#[cfg(feature = "postgres")]
mod postgres;

pub use repo::{CatalogEntry, HistoryPoint, NewUser, PriceRepo, StorageError};

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteRepo as DefaultRepo;
#[cfg(feature = "postgres")]
pub use postgres::PostgresRepo as DefaultRepo;

pub async fn connect(database_url: &str) -> Result<DefaultRepo, StorageError> {
    DefaultRepo::connect(database_url).await
}
