-- Displayed vs hidden stateroom for GTY (guarantee) bookings.
-- `stateroom` is what RCG shows ("GTY" until assigned, else the real cabin).
-- `assigned_stateroom` is the real cabin a GTY booking currently maps to,
-- recovered from purchased add-on order records.
ALTER TABLE bookings ADD COLUMN stateroom TEXT;
ALTER TABLE bookings ADD COLUMN assigned_stateroom TEXT;
