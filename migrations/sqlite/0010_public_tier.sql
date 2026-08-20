-- Public (no-login) tier. Additive: authed tables untouched. See the postgres
-- variant for the design rationale. account_scope '' == public promotional series.

CREATE TABLE sailings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    brand         TEXT NOT NULL CHECK (brand IN ('royal', 'celebrity')),
    ship_code     TEXT NOT NULL,
    sail_date     TEXT NOT NULL,
    active        INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (brand, ship_code, sail_date)
);
CREATE INDEX idx_sailings_active ON sailings(active);

ALTER TABLE bookings ADD COLUMN sailing_id INTEGER REFERENCES sailings(id) ON DELETE SET NULL;

CREATE TABLE tracked_products (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    sailing_id            INTEGER NOT NULL REFERENCES sailings(id) ON DELETE CASCADE,
    product_code          TEXT NOT NULL,
    category_prefix       TEXT NOT NULL,
    label                 TEXT,
    account_scope         TEXT NOT NULL DEFAULT '',
    active                INTEGER NOT NULL DEFAULT 1,
    consecutive_failures  INTEGER NOT NULL DEFAULT 0,
    last_fetch_at         TEXT,
    last_success_at       TEXT,
    last_error            TEXT,
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (sailing_id, product_code, account_scope)
);
CREATE INDEX idx_tracked_active ON tracked_products(active);
CREATE INDEX idx_tracked_sailing ON tracked_products(sailing_id);

CREATE TABLE sailing_price_snapshots (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    tracked_id         INTEGER NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    fetched_at         TEXT NOT NULL,
    adult_promo_price  REAL,
    child_promo_price  REAL,
    base_price         REAL,
    promo_present      INTEGER NOT NULL DEFAULT 0,
    raw_response       TEXT NOT NULL,
    UNIQUE (tracked_id, fetched_at)
);
CREATE INDEX idx_sailing_snapshots_time ON sailing_price_snapshots(tracked_id, fetched_at DESC);

CREATE TABLE sailing_diffs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tracked_id   INTEGER NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    detected_at  TEXT NOT NULL,
    old_price    REAL NOT NULL,
    new_price    REAL NOT NULL,
    delta_pct    REAL NOT NULL,
    notified     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_sailing_diffs_notified ON sailing_diffs(notified, detected_at);

CREATE TABLE public_channels (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    kind          TEXT NOT NULL CHECK (kind IN ('webpush', 'email')),
    endpoint      TEXT NOT NULL,
    p256dh        TEXT,
    auth          TEXT,
    device_token  TEXT,
    verified      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (kind, endpoint)
);
CREATE INDEX idx_public_channels_device ON public_channels(device_token);

CREATE TABLE public_subscriptions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id       INTEGER NOT NULL REFERENCES public_channels(id) ON DELETE CASCADE,
    tracked_id       INTEGER NOT NULL REFERENCES tracked_products(id) ON DELETE CASCADE,
    alert_mode       TEXT NOT NULL DEFAULT 'any_drop' CHECK (alert_mode IN ('any_drop', 'below_threshold')),
    alert_threshold  REAL,
    active           INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (channel_id, tracked_id)
);
CREATE INDEX idx_public_subs_tracked ON public_subscriptions(tracked_id);

-- Backfill sailings from existing bookings, then link them.
INSERT INTO sailings (brand, ship_code, sail_date)
SELECT DISTINCT brand, ship_code, sail_date FROM bookings
WHERE true
ON CONFLICT (brand, ship_code, sail_date) DO NOTHING;

UPDATE bookings SET sailing_id = (
    SELECT s.id FROM sailings s
    WHERE s.brand = bookings.brand
      AND s.ship_code = bookings.ship_code
      AND s.sail_date = bookings.sail_date
);
