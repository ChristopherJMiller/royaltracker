CREATE TABLE catalog_cache (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    reservation_id    TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    category_id       TEXT NOT NULL,
    category_name     TEXT NOT NULL,
    product_code      TEXT NOT NULL,
    title             TEXT NOT NULL,
    summary           TEXT,
    starting_price    REAL,
    currency          TEXT,
    fetched_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (reservation_id, category_id, product_code)
);

CREATE INDEX idx_catalog_reservation_category ON catalog_cache(reservation_id, category_id);
CREATE INDEX idx_catalog_search_title ON catalog_cache(title COLLATE NOCASE);
