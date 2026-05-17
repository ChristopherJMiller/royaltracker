CREATE TABLE booking_subscribers (
    reservation_id TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (reservation_id, user_id)
);
CREATE INDEX idx_booking_subscribers_user ON booking_subscribers(user_id);

INSERT INTO booking_subscribers (reservation_id, user_id)
SELECT reservation_id, user_id FROM bookings WHERE user_id IS NOT NULL
ON CONFLICT DO NOTHING;

CREATE TYPE alert_mode_kind AS ENUM ('any_drop', 'below_threshold');
ALTER TABLE products_watched ADD COLUMN alert_mode alert_mode_kind NOT NULL DEFAULT 'any_drop';
ALTER TABLE products_watched ADD COLUMN alert_threshold DOUBLE PRECISION;
