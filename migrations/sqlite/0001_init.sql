CREATE TABLE bookings (
    reservation_id   TEXT PRIMARY KEY,
    brand            TEXT NOT NULL CHECK (brand IN ('royal', 'celebrity')),
    account_id       TEXT NOT NULL,
    ship_code        TEXT NOT NULL,
    sail_date        TEXT NOT NULL,
    passenger_id     TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_bookings_brand ON bookings(brand);
CREATE INDEX idx_bookings_account ON bookings(account_id);

CREATE TABLE products_watched (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    reservation_id   TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    category_prefix  TEXT NOT NULL,
    product_code     TEXT NOT NULL,
    label            TEXT,
    active           INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (reservation_id, category_prefix, product_code)
);

CREATE INDEX idx_watched_active ON products_watched(active);

CREATE TABLE price_snapshots (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    watched_id           INTEGER NOT NULL REFERENCES products_watched(id) ON DELETE CASCADE,
    fetched_at           TEXT NOT NULL,
    adult_promo_price    REAL,
    child_promo_price    REAL,
    raw_response         TEXT NOT NULL,
    UNIQUE (watched_id, fetched_at)
);

CREATE INDEX idx_snapshots_watched_time ON price_snapshots(watched_id, fetched_at DESC);

CREATE TABLE diffs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    watched_id      INTEGER NOT NULL REFERENCES products_watched(id) ON DELETE CASCADE,
    detected_at     TEXT NOT NULL,
    old_price       REAL NOT NULL,
    new_price       REAL NOT NULL,
    delta_pct       REAL NOT NULL,
    notified        INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_diffs_notified ON diffs(notified, detected_at);
CREATE INDEX idx_diffs_watched ON diffs(watched_id, detected_at DESC);
