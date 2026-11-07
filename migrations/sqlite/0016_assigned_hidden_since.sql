-- The day Royal started masking a guarantee cabin's room in order history
-- (YYYY-MM-DD). While set, assigned_stateroom is the last room we saw.
ALTER TABLE bookings ADD COLUMN assigned_hidden_since TEXT;
