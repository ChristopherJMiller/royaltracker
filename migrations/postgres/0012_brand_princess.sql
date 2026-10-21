-- Princess Cruises joins the brand enum. ALTER TYPE ... ADD VALUE cannot be
-- used in the same transaction as statements that use the new value, so this
-- lives alone in its own migration; 0013 and later may then reference it.
ALTER TYPE brand_kind ADD VALUE IF NOT EXISTS 'princess';
