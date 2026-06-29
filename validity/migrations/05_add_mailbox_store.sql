-- Migration script to add mailbox store fields to the requests table

-- Add mailbox store fields to requests table
ALTER TABLE requests 
ADD COLUMN IF NOT EXISTS mailbox_inbox_chains BYTEA[],
ADD COLUMN IF NOT EXISTS mailbox_outbox_chains BYTEA[],
ADD COLUMN IF NOT EXISTS mailbox_inbox_roots BYTEA[],
ADD COLUMN IF NOT EXISTS mailbox_outbox_roots BYTEA[];

