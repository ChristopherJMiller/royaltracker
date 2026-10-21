-- One login per (user, cruise line). Credentials move out of users (whose rcg_*
-- columns stay, now nullable, for rollback safety); existing users become an
-- 'rcg' account. Never grant this table to royaltracker_public (0011's default
-- privileges already withhold it).
CREATE TABLE cruise_accounts (
    id               BIGSERIAL PRIMARY KEY,
    user_id          BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    cruise_line      TEXT NOT NULL CHECK (cruise_line IN ('rcg', 'princess')),
    login            TEXT NOT NULL,
    password_ct      BYTEA NOT NULL,
    password_nonce   BYTEA NOT NULL,
    active           BOOLEAN NOT NULL DEFAULT TRUE,
    last_login_at    TIMESTAMPTZ,
    last_login_error TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, cruise_line)
);

INSERT INTO cruise_accounts (user_id, cruise_line, login, password_ct, password_nonce, active)
SELECT id, 'rcg', rcg_username, rcg_password_ct, rcg_password_nonce, active
FROM users WHERE rcg_username IS NOT NULL;

ALTER TABLE users
    ALTER COLUMN rcg_username DROP NOT NULL,
    ALTER COLUMN rcg_password_ct DROP NOT NULL,
    ALTER COLUMN rcg_password_nonce DROP NOT NULL,
    ALTER COLUMN brand_pref DROP NOT NULL;

ALTER TABLE bookings ADD COLUMN voyage_code TEXT, ADD COLUMN currency TEXT;

-- Princess combo voyages share ship and date, so voyage_code joins the key.
ALTER TABLE sailings
    ADD COLUMN voyage_code TEXT NOT NULL DEFAULT '',
    ADD COLUMN currency TEXT;
ALTER TABLE sailings
    DROP CONSTRAINT sailings_brand_ship_code_sail_date_key,
    ADD CONSTRAINT sailings_identity_key UNIQUE (brand, ship_code, sail_date, voyage_code);

-- Deck plans are keyed by brand as well as ship. Lazily repopulated cache, so
-- it is simply emptied (needed to add the NOT NULL brand column).
DELETE FROM deck_plans;
ALTER TABLE deck_plans
    ADD COLUMN brand brand_kind NOT NULL,
    DROP CONSTRAINT deck_plans_pkey,
    ADD PRIMARY KEY (brand, ship_code, deck);

-- Link bookings that predate the sailing backfill to a sailing.
INSERT INTO sailings (brand, ship_code, sail_date, voyage_code)
SELECT DISTINCT brand, ship_code, sail_date, '' FROM bookings WHERE sailing_id IS NULL
ON CONFLICT (brand, ship_code, sail_date, voyage_code) DO NOTHING;

UPDATE bookings SET sailing_id = (
    SELECT s.id FROM sailings s
    WHERE s.brand = bookings.brand AND s.ship_code = bookings.ship_code
      AND s.sail_date = bookings.sail_date AND s.voyage_code = ''
) WHERE sailing_id IS NULL;
