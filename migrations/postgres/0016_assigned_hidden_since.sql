-- The day Royal started masking a guarantee cabin's room in order history.
-- While set, assigned_stateroom is the last room we saw.
ALTER TABLE bookings ADD COLUMN IF NOT EXISTS assigned_hidden_since DATE;
