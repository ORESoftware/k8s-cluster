-- billing-server-rs :: faster payment, Zelle, and EVM observer providers
--
-- Zelle is not a public merchant API; programmatic access is exposed through
-- bank-sponsored enterprise products. Modern Treasury and Dwolla cover ACH and
-- faster-payment rails (RTP/FedNow where tenant programs support them).
-- Ethereum is observer-mode only, mirroring solana_wallet: addresses + RPC
-- reads, never private keys or delegated spend authority.

ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'us_bank_zelle';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'jpmorgan_zelle';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'bofa_cashpro_gdd';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'modern_treasury';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'dwolla';
ALTER TYPE provider_kind ADD VALUE IF NOT EXISTS 'ethereum_wallet';
