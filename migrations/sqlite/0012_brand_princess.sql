-- Princess support, SQLite side: SQLite can't alter CHECK constraints, drop
-- NOT NULL, or change a UNIQUE key in place, so users / bookings / sailings
-- are rebuilt (the documented 12-step procedure).
--
-- This file is a plain migration: the rebuilds rely on SqliteRepo::migrate()
-- running the migrator on a connection with foreign_keys OFF (PRAGMA
-- foreign_keys is a no-op inside sqlx's per-migration transaction, and sqlx
-- 0.8's SQLite driver ignores `-- no-transaction`). With foreign keys ON,
-- DROP TABLE would cascade-delete every child row (snapshots, watches,
-- subscribers; see the comment in 0007). migrate() runs foreign_key_check
-- afterwards and fails on any violation.
--
-- Changes: users.rcg_* and brand_pref become nullable (credentials move to
-- cruise_accounts in 0013; columns kept, not dropped, so a rollback still
-- works); brand CHECKs allow 'princess'; bookings gain voyage_code/currency;
-- sailings gain voyage_code/currency and are unique per
-- (brand, ship_code, sail_date, voyage_code) since Princess combo voyages
-- share ship and date.

CREATE TABLE users_new (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    telegram_chat_id      INTEGER NOT NULL UNIQUE,
    telegram_username     TEXT,
    rcg_username          TEXT,
    rcg_password_ct       BLOB,
    rcg_password_nonce    BLOB,
    brand_pref            TEXT DEFAULT 'royal',
    active                INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO users_new (id, telegram_chat_id, telegram_username, rcg_username, rcg_password_ct,
                       rcg_password_nonce, brand_pref, active, created_at, updated_at)
SELECT id, telegram_chat_id, telegram_username, rcg_username, rcg_password_ct,
       rcg_password_nonce, brand_pref, active, created_at, updated_at FROM users;
DROP TABLE users;
ALTER TABLE users_new RENAME TO users;
CREATE INDEX idx_users_active ON users(active);

CREATE TABLE sailings_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    brand         TEXT NOT NULL CHECK (brand IN ('royal', 'celebrity', 'princess')),
    ship_code     TEXT NOT NULL,
    sail_date     TEXT NOT NULL,
    active        INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    voyage_code   TEXT NOT NULL DEFAULT '',
    currency      TEXT,
    UNIQUE (brand, ship_code, sail_date, voyage_code)
);
INSERT INTO sailings_new (id, brand, ship_code, sail_date, active, first_seen_at)
SELECT id, brand, ship_code, sail_date, active, first_seen_at FROM sailings;
DROP TABLE sailings;
ALTER TABLE sailings_new RENAME TO sailings;
CREATE INDEX idx_sailings_active ON sailings(active);

CREATE TABLE bookings_new (
    reservation_id     TEXT PRIMARY KEY,
    brand              TEXT NOT NULL CHECK (brand IN ('royal', 'celebrity', 'princess')),
    account_id         TEXT NOT NULL,
    ship_code          TEXT NOT NULL,
    sail_date          TEXT NOT NULL,
    passenger_id       TEXT,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
    user_id            INTEGER REFERENCES users(id) ON DELETE CASCADE,
    nights             INTEGER,
    package_code       TEXT,
    stateroom          TEXT,
    assigned_stateroom TEXT,
    sailing_id         INTEGER REFERENCES sailings(id) ON DELETE SET NULL,
    voyage_code        TEXT,
    currency           TEXT
);
INSERT INTO bookings_new (reservation_id, brand, account_id, ship_code, sail_date, passenger_id,
                          created_at, updated_at, user_id, nights, package_code, stateroom,
                          assigned_stateroom, sailing_id)
SELECT reservation_id, brand, account_id, ship_code, sail_date, passenger_id,
       created_at, updated_at, user_id, nights, package_code, stateroom,
       assigned_stateroom, sailing_id FROM bookings;
DROP TABLE bookings;
ALTER TABLE bookings_new RENAME TO bookings;
CREATE INDEX idx_bookings_brand ON bookings(brand);
CREATE INDEX idx_bookings_account ON bookings(account_id);
CREATE INDEX idx_bookings_user ON bookings(user_id);
