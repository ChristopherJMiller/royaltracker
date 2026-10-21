-- One login per (user, cruise line). Credentials move out of users (whose rcg_*
-- columns stay, now nullable, for rollback safety); existing users become an
-- 'rcg' account. Never grant this table to royaltracker_public (0011's default
-- privileges already withhold it).
CREATE TABLE cruise_accounts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    cruise_line      TEXT NOT NULL CHECK (cruise_line IN ('rcg', 'princess')),
    login            TEXT NOT NULL,
    password_ct      BLOB NOT NULL,
    password_nonce   BLOB NOT NULL,
    active           INTEGER NOT NULL DEFAULT 1,
    last_login_at    TEXT,
    last_login_error TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, cruise_line)
);

INSERT INTO cruise_accounts (user_id, cruise_line, login, password_ct, password_nonce, active)
SELECT id, 'rcg', rcg_username, rcg_password_ct, rcg_password_nonce, active
FROM users WHERE rcg_username IS NOT NULL;

-- Deck plans are keyed by brand as well as ship. It's a lazily repopulated
-- cache, so it is simply emptied. Nothing references it, so DROP is FK-safe.
DROP TABLE deck_plans;
CREATE TABLE deck_plans (
    brand      TEXT NOT NULL CHECK (brand IN ('royal', 'celebrity', 'princess')),
    ship_code  TEXT NOT NULL,
    deck       INTEGER NOT NULL,
    image_url  TEXT NOT NULL,
    sourced_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (brand, ship_code, deck)
);

-- Link bookings that predate the sailing backfill to a sailing.
INSERT INTO sailings (brand, ship_code, sail_date, voyage_code)
SELECT DISTINCT brand, ship_code, sail_date, '' FROM bookings WHERE sailing_id IS NULL
ON CONFLICT (brand, ship_code, sail_date, voyage_code) DO NOTHING;

UPDATE bookings SET sailing_id = (
    SELECT s.id FROM sailings s
    WHERE s.brand = bookings.brand AND s.ship_code = bookings.ship_code
      AND s.sail_date = bookings.sail_date AND s.voyage_code = ''
) WHERE sailing_id IS NULL;
