-- Cabin details some lines report alongside the cabin number. Princess names
-- its decks ("Dolphin") and describes the category ("Deluxe Balcony").
-- RCG decks are numbered and derived from the cabin, so these stay NULL there.
ALTER TABLE bookings ADD COLUMN deck_name TEXT;
ALTER TABLE bookings ADD COLUMN cabin_category TEXT;
