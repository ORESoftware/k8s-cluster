# Memebank integration contract

Memebank is a companion image and meme catalog for ClipTown. Memebank owns image ingestion, source synchronization, OCR, visual tagging, semantic search, collections, and sharing policy. ClipTown owns generic clipboard history, retention, encryption, and trusted-device synchronization.

The two products integrate through explicit clipboard writes and versioned public contracts. They do not share databases or cloud-storage credentials.

## Clipboard representations

When a user invokes **Copy** in Memebank, the client should write the richest safe set of representations supported by the current platform:

1. standard image bytes such as `image/png` or `image/webp`;
2. a temporary local file reference where the operating system supports file clipboard entries;
3. `text/plain` containing an authorized share URL, or a `memebank://assets/{asset-id}` deep link when no share URL exists;
4. optional `application/vnd.memebank.asset+json` metadata for Memebank-aware consumers.

Example metadata:

```json
{
  "schema": "memebank.clipboard/v1",
  "assetId": "01900000-0000-7000-8000-000000000001",
  "mediaType": "image/png",
  "sha256": "hex-encoded-sha256",
  "title": "Distracted boyfriend",
  "source": "memebank",
  "deepLink": "memebank://assets/01900000-0000-7000-8000-000000000001"
}
```

The metadata flavor is additive. Paste targets that know nothing about Memebank must still receive a standard image, file, or text representation.

## ClipTown behavior

ClipTown may capture the standard clipboard representation under its existing consent, encryption, retention, and device-sync rules. It may retain the Memebank metadata as provenance, but must not depend on the Memebank API to keep the copied item usable.

ClipTown must not automatically turn a private Memebank asset into a public URL. A share URL is included only when the user has already authorized sharing.

## Security and privacy invariants

- No bearer token, refresh token, storage credential, signed upload URL, provider OAuth token, private object-store URL, or encryption key may enter clipboard metadata.
- Clipboard export is an explicit foreground action. Neither product background-scrapes the clipboard to infer activity in the other.
- Memebank and ClipTown do not read one another's private databases.
- The copied image should be materialized locally before the clipboard write; expiring remote URLs are not a substitute for standard image bytes when the platform supports them.
- Temporary files must have bounded lifetime and restrictive permissions.
- A future direct local bridge must use authenticated platform IPC or a loopback protocol, remain optional, and be covered by a separately versioned interface.

## Shared authentication

Both products may consume `shared-auth`, but each service validates its own audience and scopes. A valid ClipTown access token is not automatically valid for Memebank, and vice versa. Cross-product actions require an explicitly issued audience/scope or a local user-mediated handoff.

## Versioning

The canonical Memebank clipboard schema belongs in `memebank/mb-interfaces`. ClipTown should treat unknown additive fields as optional and reject unsupported major schema versions. Material contract changes require coordinated interface and implementation pull requests in both organizations.
