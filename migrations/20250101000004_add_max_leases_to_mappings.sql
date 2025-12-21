-- Add max_leases column to user_asn_mappings table
-- This allows per-user configuration of maximum concurrent prefix leases

ALTER TABLE user_asn_mappings
ADD COLUMN max_leases INTEGER NOT NULL DEFAULT 1;

-- Add a check constraint to ensure max_leases is positive
ALTER TABLE user_asn_mappings
ADD CONSTRAINT max_leases_positive CHECK (max_leases > 0);
