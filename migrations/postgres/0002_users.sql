CREATE TABLE users (
    id                    BIGSERIAL PRIMARY KEY,
    telegram_chat_id      BIGINT NOT NULL UNIQUE,
    telegram_username     TEXT,
    rcg_username          TEXT NOT NULL,
    rcg_password_ct       BYTEA NOT NULL,
    rcg_password_nonce    BYTEA NOT NULL,
    brand_pref            brand_kind NOT NULL DEFAULT 'royal',
    active                BOOLEAN NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_active ON users(active);

ALTER TABLE bookings ADD COLUMN user_id BIGINT REFERENCES users(id) ON DELETE CASCADE;
CREATE INDEX idx_bookings_user ON bookings(user_id);
