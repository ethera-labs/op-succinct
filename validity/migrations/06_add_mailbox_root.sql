-- Migration script to add mailbox root field to the requests table

-- Add mailbox root field to requests table
ALTER TABLE requests 
ADD COLUMN IF NOT EXISTS mailbox_root BYTEA;
