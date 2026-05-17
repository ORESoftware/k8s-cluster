# Generated sqlc adapter

This directory is a self-contained sqlc workspace. Run `sqlc generate` from inside this folder
to produce the typed Go bindings; the canonical DDL is mirrored from
`remote/libs/pg-defs/schema/schema.sql`. The query catalogue in `query.sql` is a starter set of
list/get/create/update/delete queries — extend it inside your service rather than here.

> Never apply `schema.sql` from this directory to a real database; this copy exists solely so that
> `sqlc` can introspect the schema offline. Use the pg-defs diff workflow for migrations.
