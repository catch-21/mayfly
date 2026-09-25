# Why every record is a JWS

Every Mayfly record — a link, a confirmation, a reject, a watchman receipt — is a JWS compact
string: `base64url(header).base64url(payload).base64url(signature)`, header
`{"alg":"EdDSA","typ":"mayfly-<type>"}`. The signed bytes are the stored bytes. This document
says why, and what that buys a verifier.

## The property

A record proves who signed it without asking anyone. The signature verifies under a key the
record names, and that key is bound to a pubky by a Grant the pubky's identity key signed
(spec §5). A verifier checks the Grant under the pubky, checks the Grant names the signing
key, and checks the record under that key. Three steps, all offline.

That is the property a homeserver cannot give. Where a file sits is not proof of who wrote it:
a homeserver can be asked to serve anything, and a mirror holds other people's records by
design. The proof travels in the bytes, so a record copied to a mirror, a cache, a message or
a disk image still proves its signer, and a record whose signer cannot be shown fails wherever
it is found. A bystander with the chain URL and nothing else can verify every write.

## What it costs, and why it is worth it

- **Opaque on disk.** A generic file browser shows a compact JWS as an opaque string. That is
  deliberate. Storing the payload in clear makes the signed bytes a substring of the file, and
  a verifier then needs the exact byte range of the payload — `JSON.parse` does not give it —
  or it re-serialises and reintroduces the canonicalisation problem. The explorer decodes the
  record, checks the signature and the hash, and shows the raw JWS beside them (§14).
- **One hash, three spellings.** `BLAKE3(file bytes)` is the homeserver `ETag`, the SSE
  `content_hash`, and the hash in the file name, spelled three ways (§6). Implementations
  decode to bytes and compare bytes.
- **Domain separation.** A record's `typ` is `mayfly-<type>`, so a chain record can never be
  replayed as a Pubky session proof, and the SDK refuses to sign a `pubky-*` type with the
  same key (§5.3).

The chain's own checks — the hash link to the previous record, the quorum of confirmations —
are separate and unchanged. The JWS is what makes each of those records attributable on its
own, wherever it is kept.
