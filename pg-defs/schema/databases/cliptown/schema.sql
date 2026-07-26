-- Cliptown Supabase Declarative Schema
-- Defines the namespace, core tables, and Row Level Security for E2EE clipping.

CREATE SCHEMA IF NOT EXISTS cliptown;

-- The clips table stores the E2EE clipboard data.
-- Since the content is encrypted client-side, the server cannot read it.
-- We use auth.users to enforce strict RLS.
CREATE TABLE IF NOT EXISTS cliptown.clips (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    encrypted_content TEXT NOT NULL,
    nonce TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

-- Enable RLS to ensure users can only access their own clips.
ALTER TABLE cliptown.clips ENABLE ROW LEVEL SECURITY;

-- Policy: Users can see only their own clips.
CREATE POLICY "Users can only access their own clips"
ON cliptown.clips
FOR ALL
USING (auth.uid() = user_id);
