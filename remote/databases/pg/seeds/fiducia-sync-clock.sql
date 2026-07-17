-- Seed the fiducia plane's transactional sync-clock singleton.
-- The row MUST exist before any write to a fiducia synced table: the
-- fiducia.lock_sync_clock / fiducia.bump_row_version triggers raise if it is
-- missing (they never create it). Idempotent; never resets an advanced clock.
-- Schema DDL lives in remote/libs/pg-defs/schema/schema.sql (fiducia section).

insert into fiducia.sync_clock (singleton, last_sequence)
values (true, 0)
on conflict (singleton) do nothing;
