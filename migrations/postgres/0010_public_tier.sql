-- Public (no-login) tier. Additive: the reservation-centric authed tables
-- (bookings/products_watched/price_snapshots/diffs) are untouched. Price series
-- are re-keyed on a shared `sailing` (brand+ship+date) so ONE public-promo fetch
-- per (sailing, product) feeds every subscriber. account_scope '' == the public
-- promotional series; a non-empty scope is reserved for personalized variants.

CREATE TABLE sailings (
    id            BIGSERIAL PRIMARY KEY,
    brand         brand_kind NOT NULL,
    ship_code     TEXT NOT NULL,
    sail_date     DATE NOT NULL,
    active        BOOLEAN NOT NULL DEFAULT TRUE,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (brand, ship_code, sail_date)
);
CREATE INDEX idx_sailings_active ON sailings(active);

-- Link existing bookings to a sailing (nullable; backfilled below).
ALTER TABLE bookings ADD COLUMN sailing_id BIGINT REFERENCES sailings(id) ON DELETE SET NULL;

CREATE TABLE tracked_products (
    id                    BIGSERIAL PRIMARY KEY,
    sailing_id            BIGINT NOT NULL REFERENCES sailings(id) ON DELETE CASCADE,
    product_code          TEXT NOT NULL,
    category_prefix       TEXT NOT NULL,
    label                 TEXT,
    account_scope         TEXT NOT NULL DEFAULT '',
    active                BOOLEAN NOT NULL DEFAULT TRUE,
    consecutive_failures  INTEGER NOT NULL DEFAULT 0,
    last_fetch_at         TIMESTAMPTZ,
    last_success_at       TIMESTAMPTZ,
    last_error            TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (sailing_id, product_code, account_scope)
);
CREATE INDEX idx_tracked_active ON tracked_products(active);
CREATE INDEX idx_tracked_sailing ON tracked_products(sailing_id);

CREATE TABLE sailing_price_snapshots (
    id                 BIGSERIAL PRIMARY KEY,
    tracked_id         BIGINT NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    fetched_at         TIMESTAMPTZ NOT NULL,
    adult_promo_price  DOUBLE PRECISION,
    child_promo_price  DOUBLE PRECISION,
    base_price         DOUBLE PRECISION,
    promo_present      BOOLEAN NOT NULL DEFAULT FALSE,
    raw_response       JSONB NOT NULL,
    UNIQUE (tracked_id, fetched_at)
);
CREATE INDEX idx_sailing_snapshots_time ON sailing_price_snapshots(tracked_id, fetched_at DESC);

CREATE TABLE sailing_diffs (
    id           BIGSERIAL PRIMARY KEY,
    tracked_id   BIGINT NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    detected_at  TIMESTAMPTZ NOT NULL,
    old_price    DOUBLE PRECISION NOT NULL,
    new_price    DOUBLE PRECISION NOT NULL,
    delta_pct    DOUBLE PRECISION NOT NULL,
    notified     BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX idx_sailing_diffs_notified ON sailing_diffs(notified, detected_at);

-- Unified public identity table. web-push registrations are rows with
-- kind='webpush'; email subscribers are kind='email' (endpoint == address).
-- Telegram is NOT stored here.
CREATE TYPE channel_kind AS ENUM ('webpush', 'email');

CREATE TABLE public_channels (
    id            BIGSERIAL PRIMARY KEY,
    kind          channel_kind NOT NULL,
    endpoint      TEXT NOT NULL,
    p256dh        TEXT,
    auth          TEXT,
    device_token  TEXT,
    verified      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kind, endpoint)
);
CREATE INDEX idx_public_channels_device ON public_channels(device_token);

CREATE TABLE public_subscriptions (
    id               BIGSERIAL PRIMARY KEY,
    channel_id       BIGINT NOT NULL REFERENCES public_channels(id) ON DELETE CASCADE,
    tracked_id       BIGINT NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    alert_mode       alert_mode_kind NOT NULL DEFAULT 'any_drop',
    alert_threshold  DOUBLE PRECISION,
    active           BOOLEAN NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (channel_id, tracked_id)
);
CREATE INDEX idx_public_subs_tracked ON public_subscriptions(tracked_id);

-- Backfill sailings from existing bookings, then link them.
INSERT INTO sailings (brand, ship_code, sail_date)
SELECT DISTINCT brand, ship_code, sail_date FROM bookings
ON CONFLICT (brand, ship_code, sail_date) DO NOTHING;

UPDATE bookings b SET sailing_id = s.id
FROM sailings s
WHERE s.brand = b.brand AND s.ship_code = b.ship_code AND s.sail_date = b.sail_date;
