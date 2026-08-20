-- Roles/grants are Postgres-only. Dev SQLite is single-tenant with no role wall,
-- so this migration is intentionally empty (present to keep the migration numbers
-- aligned across dialects). See migrations/postgres/0011_public_role_grants.sql.
SELECT 1;
