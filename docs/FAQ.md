# FAQ

## What is Mayfly?

Mayfly is a protocol and engine for a small, fixed set of parties to keep a
shared, verifiable history on Pubky. Each record is a signed, hash-linked link
on its author's own homeserver, and the next record can be proposed only after
that round's confirmations reach the quorum fixed when the chain began.

## What problem does Mayfly solve?

It lets those parties agree what happened, and in what order, with no shared
server and no operator in the middle. Anyone who can read the parties' public
storage can replay the files and check that every link was signed by an
established party, confirmed before the chain moved on, and left unchanged.

## How does Mayfly use grant auth and JWS to enable verification?

The identity key in Pubky Ring signs a Grant JWS that names the app's client
key, and Mayfly uses that client key to sign every record as its own JWS. A
verifier checks the Grant under the party's pubky, checks that the client key
is the one that signed the record, and checks the record itself, using only
the public keys carried in the chain.

## Why use a watchman, and how?

The parties can show each other what they agreed and in which order; a
watchman is the impartial observer that timestamps each record and makes a
later rewrite visible. Genesis names it, it publishes a signed engagement and
a receipt for every record it sees, and a verifier reads those receipts for
questions of time while the chain itself advances on the parties' votes.

## Can I build a Mayfly app?

Yes. An app supplies a versioned, deterministic rules function — current state
and a link in, next state out — and the Mayfly client proposes, confirms, and
verifies links on each party's Pubky storage. A shared list, chess, and an
agreed document are built this way; a new app is a new rules id pinned in
genesis, plus a page that drives the client.

## How does Mayfly ensure witnessed data integrity?

Every committed link is mirrored to the other parties' homeservers, and each
record is signed and hash-linked, so any of those copies verifies on its own
if the author later alters or deletes the file on theirs. A watchman always
keeps a signed receipt of each record's hash; keeping the bytes themselves is
an optional `mirror` service, used when the chain needs a backup beyond the
parties' own copies.
