-- Migration to create user ASN mappings table
-- This table stores the mapping between users and their assigned ASN

CREATE TABLE IF NOT EXISTS user_asn_mappings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_hash VARCHAR(64) UNIQUE NOT NULL,
    asn INTEGER UNIQUE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Create index on user_hash for efficient lookups
CREATE INDEX IF NOT EXISTS idx_user_asn_mappings_user_hash
ON user_asn_mappings (user_hash);

-- Create index on asn for efficient lookups
CREATE INDEX IF NOT EXISTS idx_user_asn_mappings_asn
ON user_asn_mappings (asn);
