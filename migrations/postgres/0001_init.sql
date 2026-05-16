CREATE TYPE brand_kind AS ENUM ('royal', 'celebrity');

CREATE TABLE bookings (
    reservation_id   TEXT PRIMARY KEY,
    brand            brand_kind NOT NULL,
    account_id       TEXT NOT NULL,
    ship_code        TEXT NOT NULL,
    sail_date        DATE NOT NULL,
    passenger_id     TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_bookings_brand ON bookings(brand);
CREATE INDEX idx_bookings_account ON bookings(account_id);

CREATE TABLE products_watched (
    id               BIGSERIAL PRIMARY KEY,
    reservation_id   TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    category_prefix  TEXT NOT NULL,
    product_code     TEXT NOT NULL,
    label            TEXT,
    active           BOOLEAN NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (reservation_id, category_prefix, product_code)
);

CREATE INDEX idx_watched_active ON products_watched(active);

CREATE TABLE price_snapshots (
    id                   BIGSERIAL PRIMARY KEY,
    watched_id           BIGINT NOT NULL REFERENCES products_watched(id) ON DELETE CASCADE,
    fetched_at           TIMESTAMPTZ NOT NULL,
    adult_promo_price    DOUBLE PRECISION,
    child_promo_price    DOUBLE PRECISION,
    raw_response         JSONB NOT NULL,
    UNIQUE (watched_id, fetched_at)
);

CREATE INDEX idx_snapshots_watched_time ON price_snapshots(watched_id, fetched_at DESC);

CREATE TABLE diffs (
    id              BIGSERIAL PRIMARY KEY,
    watched_id      BIGINT NOT NULL REFERENCES products_watched(id) ON DELETE CASCADE,
    detected_at     TIMESTAMPTZ NOT NULL,
    old_price       DOUBLE PRECISION NOT NULL,
    new_price       DOUBLE PRECISION NOT NULL,
    delta_pct       DOUBLE PRECISION NOT NULL,
    notified        BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_diffs_notified ON diffs(notified, detected_at);
CREATE INDEX idx_diffs_watched ON diffs(watched_id, detected_at DESC);
