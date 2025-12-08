-- Migration to create prefix leases table
-- This table stores the prefix leases for users with time-based expiration

CREATE TABLE IF NOT EXISTS prefix_leases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_hash VARCHAR(64) NOT NULL,
    prefix CIDR NOT NULL,
    start_time TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    end_time TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Create index on user_hash for efficient lookups
CREATE INDEX IF NOT EXISTS idx_prefix_leases_user_hash
ON prefix_leases (user_hash);

-- Create index on prefix for efficient lookups
CREATE INDEX IF NOT EXISTS idx_prefix_leases_prefix
ON prefix_leases (prefix);

-- Create index on end_time for efficient expiration queries
CREATE INDEX IF NOT EXISTS idx_prefix_leases_end_time
ON prefix_leases (end_time);

-- Create composite index for active leases lookup
CREATE INDEX IF NOT EXISTS idx_prefix_leases_active
ON prefix_leases (prefix, end_time);
