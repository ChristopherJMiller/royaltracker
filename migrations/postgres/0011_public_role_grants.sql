-- Least-privilege grants for the public web service's DB role.
--
-- The role `royaltracker_public` is created out-of-band (the Zalando operator's
-- psql.yaml `users:` block), so this migration is a no-op when the role is
-- absent (dev, or before the role exists). Runs as the owner at boot, so it has
-- grant authority over these objects. Idempotent — safe to re-run.
--
-- The public service does: read prices, seed-on-lookup (upsert sailings /
-- tracked_products, record snapshots), and manage subscriptions. It must NEVER
-- reach the authed tables (encrypted RCG credentials, bookings, watches).

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'royaltracker_public') THEN
        -- Schema usage.
        GRANT USAGE ON SCHEMA public TO royaltracker_public;

        -- Public-tier tables the service reads AND writes (seed-on-lookup + subscribe).
        GRANT SELECT, INSERT, UPDATE ON
            sailings, tracked_products, sailing_price_snapshots,
            public_channels, public_subscriptions
            TO royaltracker_public;
        -- Diffs are written by the sweep (owner); the public service only reads.
        GRANT SELECT ON sailing_diffs TO royaltracker_public;

        -- Sequences behind the BIGSERIAL ids the service inserts.
        GRANT USAGE, SELECT ON
            sailings_id_seq, tracked_products_id_seq, sailing_price_snapshots_id_seq,
            public_channels_id_seq, public_subscriptions_id_seq
            TO royaltracker_public;

        -- Hard wall around the authed tier (defence in depth vs the app layer).
        REVOKE ALL ON
            users, bookings, booking_subscribers, products_watched,
            price_snapshots, diffs, catalog_cache, deck_plans
            FROM royaltracker_public;

        -- Deny-by-default for anything created later by the owner.
        ALTER DEFAULT PRIVILEGES IN SCHEMA public
            REVOKE ALL ON TABLES FROM royaltracker_public;
        ALTER DEFAULT PRIVILEGES IN SCHEMA public
            REVOKE ALL ON SEQUENCES FROM royaltracker_public;
    END IF;
END
$$;
