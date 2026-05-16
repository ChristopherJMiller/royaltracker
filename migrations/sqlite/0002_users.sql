CREATE TABLE users (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    telegram_chat_id      INTEGER NOT NULL UNIQUE,
    telegram_username     TEXT,
    rcg_username          TEXT NOT NULL,
    rcg_password_ct       BLOB NOT NULL,
    rcg_password_nonce    BLOB NOT NULL,
    brand_pref            TEXT NOT NULL DEFAULT 'royal' CHECK (brand_pref IN ('royal', 'celebrity')),
    active                INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_users_active ON users(active);

ALTER TABLE bookings ADD COLUMN user_id INTEGER REFERENCES users(id) ON DELETE CASCADE;
CREATE INDEX idx_bookings_user ON bookings(user_id);
