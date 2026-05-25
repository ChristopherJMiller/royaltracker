-- A product can be listed under several catalog categories at once (e.g. the
-- "Ultimate Bundle" appears under drinks, wifi AND packages), so the same
-- product could be watched once per category — duplicate rows that fetch the
-- same personalized price, double the daily scrape, and fire duplicate drop
-- alerts. The personalized price endpoint returns the same price for any of a
-- product's categories (category is just a path segment), so the real identity
-- of a watch is (reservation_id, product_code), not the category.

-- 1. Collapse existing duplicates. Keep one row per (reservation_id,
--    product_code): prefer an active row, then the one with the most price
--    history, then the lowest id. Deleting the losers cascades their snapshots
--    and diffs (which are redundant — the keeper has the same daily prices).
WITH ranked AS (
    SELECT pw.id,
           row_number() OVER (
               PARTITION BY pw.reservation_id, pw.product_code
               ORDER BY pw.active DESC,
                        (SELECT count(*) FROM price_snapshots s WHERE s.watched_id = pw.id) DESC,
                        pw.id ASC
           ) AS rn
    FROM products_watched pw
)
DELETE FROM products_watched WHERE id IN (SELECT id FROM ranked WHERE rn > 1);

-- 2. Swap the uniqueness key from (reservation_id, category_prefix,
--    product_code) to (reservation_id, product_code). The old constraint name
--    is Postgres's auto-generated one (truncated to 63 chars).
ALTER TABLE products_watched
    DROP CONSTRAINT IF EXISTS products_watched_reservation_id_category_prefix_product_cod_key;
ALTER TABLE products_watched
    ADD CONSTRAINT products_watched_reservation_product_key UNIQUE (reservation_id, product_code);
