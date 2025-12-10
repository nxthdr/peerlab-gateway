-- Migration to add user_id column to user_asn_mappings table
-- This stores the original Logto user ID for email retrieval

ALTER TABLE user_asn_mappings
ADD COLUMN IF NOT EXISTS user_id VARCHAR(255);

-- Create index on user_id for efficient lookups
CREATE INDEX IF NOT EXISTS idx_user_asn_mappings_user_id
ON user_asn_mappings (user_id);
