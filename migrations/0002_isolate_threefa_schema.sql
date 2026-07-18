-- Isolate 3FA's tables from other services sharing the cluster Postgres.
--
-- This migration deliberately moves the existing tables instead of editing
-- 0001: migration runners record checksums, so changing an already-applied file
-- would make safe, reviewed upgrades fail.
CREATE SCHEMA IF NOT EXISTS threefa;

DO $$
BEGIN
    IF to_regclass('threefa.accounts') IS NULL
       AND to_regclass('public.accounts') IS NOT NULL THEN
        ALTER TABLE public.accounts SET SCHEMA threefa;
    END IF;
    IF to_regclass('threefa.devices') IS NULL
       AND to_regclass('public.devices') IS NOT NULL THEN
        ALTER TABLE public.devices SET SCHEMA threefa;
    END IF;
    IF to_regclass('threefa.vault_blobs') IS NULL
       AND to_regclass('public.vault_blobs') IS NOT NULL THEN
        ALTER TABLE public.vault_blobs SET SCHEMA threefa;
    END IF;
END
$$;

-- The shared pg-defs adapters represent opaque binary values as base64 text so
-- every generated language binding has the same lossless contract. Convert the
-- legacy bytea columns only when they still have their original type.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'threefa'
          AND table_name = 'vault_blobs'
          AND column_name = 'ciphertext'
          AND data_type = 'bytea'
    ) THEN
        ALTER TABLE threefa.vault_blobs
            ALTER COLUMN ciphertext TYPE TEXT USING encode(ciphertext, 'base64'),
            ALTER COLUMN nonce TYPE TEXT USING encode(nonce, 'base64'),
            ALTER COLUMN kdf_salt TYPE TEXT USING encode(kdf_salt, 'base64');
    END IF;
END
$$;
