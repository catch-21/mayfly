# Principles

The specification is `docs/MAYFLY.md`. The tests in `crates/core` hold the chain rules below.
The rest are choices the design will not trade away. An application relies on all of them as
they stand. It does not rebuild them, and it does not work around them.

## There is no shared server

Each party writes only on their own homeserver. Everyone else reads. A verifier needs no
account: the files, and the public keys inside them, are enough.

A folder shows who was allowed to put a file there. The signature is what proves the record.
Homeservers can lose files, edit them, or invent them, so a record is never believed because
of the folder it was found in. Every party keeps a copy of what they have seen. A record is
recognised by the hash of its bytes, not by its name.

The people on a chain are named when it starts. Joining is their agreement to those people,
to the rules, and to what happens if someone stops answering. You always know whom you are
waiting for. A chain is for a handful of people, not a crowd.

## One link at a time

The chain is a single line. Someone proposes the next link. The others confirm it or refuse
it. A person votes once in a round, and a vote is not taken back.

Only those confirmations decide which link is committed. A round that died does not, and
neither does a watchman's receipt.

The newest link is still open. It becomes final when the link after it carries its
confirmations, and nothing after that can reopen it. Until then, treat it as provisional.

When a round can no longer succeed, it ends, and the next round belongs to one designated
proposer. Honest parties meet again within two rounds.

## The watchman stands apart

The watchman is not one of the parties. It signs for the bytes it saw and the time it saw
them. If a homeserver later changes a record, or loses it, the receipt still names what was
written. A watchman that keeps a mirror holds those bytes, so the chain can be read from
that copy when a party's own copy is gone. It cannot invent a record. It can only keep what
someone signed.

The parties do not wait for the watchman. A receipt never makes a record valid, and it never
makes one invalid.

A question of time stays open until the records it is about are final. The answer takes the
watchmen agreeing with each other. One receipt is not enough.

Without a watchman, both the time and the surviving copy rest on the parties who are still
there.

## What a chain can show

The chain shows what the parties agreed, and the order they agreed it in. Two parties who
both sign a falsehood have recorded that falsehood. The chain does not correct them.

The rules are a pure function of the records. Two honest readers of the same files reach the
same state.

When every confirmation is required, one absent party stops the chain. The others may record
that this person has left. That is the only decision taken without them.

Records stay small. A large file lives somewhere else, and the chain names it by its hash.
What is published under `/pub/` is public. This version has no private chains.
