-- Mirror of the Postgres 0007 migration: make (reservation_id, product_code)
-- the identity of a watch so the same product can't be tracked once per catalog
-- category. See the Postgres copy for the full rationale.

-- 1. Collapse existing duplicates, keeping one row per (reservation_id,
--    product_code): prefer active, then most price history, then lowest id.
DELETE FROM products_watched WHERE id IN (
    SELECT id FROM (
        SELECT pw.id,
               row_number() OVER (
                   PARTITION BY pw.reservation_id, pw.product_code
                   ORDER BY pw.active DESC,
                            (SELECT count(*) FROM price_snapshots s WHERE s.watched_id = pw.id) DESC,
                            pw.id ASC
               ) AS rn
        FROM products_watched pw
    ) WHERE rn > 1
);

-- 2. Add the stricter unique index. SQLite can't drop the original
--    UNIQUE(reservation_id, category_prefix, product_code) without a full table
--    rebuild (which, with foreign_keys enabled, would cascade-delete every
--    snapshot/diff), so we leave it: it's a superset of the new key and is
--    automatically satisfied whenever this one holds. The upsert's
--    ON CONFLICT(reservation_id, product_code) targets this index.
CREATE UNIQUE INDEX idx_watched_reservation_product
    ON products_watched(reservation_id, product_code);
