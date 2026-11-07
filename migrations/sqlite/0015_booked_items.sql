-- What each booking's cabin has bought, from Cruise Planner order history
-- (paid_total = the cabin's total per unit). Rewritten by the daily scraper.
CREATE TABLE booked_items (
    reservation_id   TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    product_code     TEXT NOT NULL,
    category_prefix  TEXT NOT NULL,
    title            TEXT NOT NULL,
    unit             TEXT NOT NULL CHECK (unit IN ('day', 'night', 'each')),
    adults           INTEGER NOT NULL,
    children         INTEGER NOT NULL,
    paid_total       REAL NOT NULL,
    paid_each        REAL,
    currency         TEXT,
    last_alert_total REAL,
    orders_read_at   TEXT NOT NULL,
    PRIMARY KEY (reservation_id, product_code)
);

-- alert_on_paid: alert when the price drops below what the cabin paid (kept
-- separate from alert_mode, whose CHECK can't gain a value in place).
-- dismissed_at: the user removed the watch; the scraper must not re-add it.
ALTER TABLE products_watched ADD COLUMN alert_on_paid INTEGER NOT NULL DEFAULT 0;
ALTER TABLE products_watched ADD COLUMN dismissed_at TEXT;
