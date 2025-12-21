-- Add revoked_at column to prefix_leases table for soft delete
-- This allows tracking when a lease was manually revoked while preserving the original end_time

ALTER TABLE prefix_leases
ADD COLUMN revoked_at TIMESTAMPTZ DEFAULT NULL;

-- Add index for efficient querying of active (non-revoked) leases
CREATE INDEX idx_prefix_leases_revoked_at ON prefix_leases(revoked_at) WHERE revoked_at IS NULL;
