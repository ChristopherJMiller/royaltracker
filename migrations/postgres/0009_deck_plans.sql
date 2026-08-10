-- Cache of resolved cruisedeckplans deck-plan image URLs, keyed by ship + deck.
-- Deck plans effectively never change, so this is populated lazily on first view
-- and reused; `sourced_at` allows an optional future refresh.
CREATE TABLE deck_plans (
    ship_code  TEXT NOT NULL,
    deck       INTEGER NOT NULL,
    image_url  TEXT NOT NULL,
    sourced_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (ship_code, deck)
);
