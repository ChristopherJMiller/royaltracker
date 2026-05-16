CREATE TABLE catalog_cache (
    id                BIGSERIAL PRIMARY KEY,
    reservation_id    TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    category_id       TEXT NOT NULL,
    category_name     TEXT NOT NULL,
    product_code      TEXT NOT NULL,
    title             TEXT NOT NULL,
    summary           TEXT,
    starting_price    DOUBLE PRECISION,
    currency          TEXT,
    fetched_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (reservation_id, category_id, product_code)
);

CREATE INDEX idx_catalog_reservation_category ON catalog_cache(reservation_id, category_id);
CREATE INDEX idx_catalog_search_title ON catalog_cache USING gin (to_tsvector('english', title));
