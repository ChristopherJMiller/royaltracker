-- Booking ownership becomes many-to-many. Previously bookings.user_id was the
-- single "owner", which meant when two users on the same reservation both ran
-- discovery the second user's upsert clobbered the first user's ownership.
CREATE TABLE booking_subscribers (
    reservation_id TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (reservation_id, user_id)
);
CREATE INDEX idx_booking_subscribers_user ON booking_subscribers(user_id);

INSERT OR IGNORE INTO booking_subscribers (reservation_id, user_id)
SELECT reservation_id, user_id FROM bookings WHERE user_id IS NOT NULL;

-- Per-watch alert configuration: either notify on any meaningful drop (legacy
-- behavior) or only when the price crosses below a user-supplied threshold.
ALTER TABLE products_watched ADD COLUMN alert_mode TEXT NOT NULL DEFAULT 'any_drop'
    CHECK (alert_mode IN ('any_drop', 'below_threshold'));
ALTER TABLE products_watched ADD COLUMN alert_threshold REAL;
