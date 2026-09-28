# FAQ

Short answers. The specification is [MAYFLY.md](MAYFLY.md).

## What is Mayfly?

Mayfly is a protocol and engine for a small, fixed set of parties to keep a
shared, verifiable history on Pubky. Each record is a signed, hash-linked link
on its author's own homeserver, and the next record can be proposed only after
the others have confirmed the one before it.

## What problem does Mayfly solve?

It lets those parties agree what happened, and in what order, with no shared
server and no operator in the middle. Anyone who can read the parties' public
storage can replay the files and check that every link was signed by an
established party, confirmed before the chain moved on, and left unchanged.

## How is Mayfly different from a blockchain?

There are no blocks, miners, or timers; the chain advances one link at a time,
only when the named parties confirm it. It suits a small number of known
parties rather than open membership.

## How many parties can take part, and can someone join later?

The parties are named when the chain starts, and the set does not grow
afterwards. Requiring everyone's confirmation suits about two to eight,
because each extra party can hold the chain by being offline; the protocol
itself only refuses fewer than two, and a larger group may set a quorum above
half.

## Who can read a chain?

Anything a party publishes under `/pub/` is public, and a verifier needs no
account. This version of Mayfly has no private chains: the files, and the
public keys inside them, are enough.

## Does a chain prove that what was recorded is true?

It proves what the parties agreed, and the order they agreed it in. Two
parties who both sign a falsehood have recorded that falsehood, and the chain
does not correct them.

## How do I verify a chain myself?

Open its chain URL in the viewer, or fetch the files from any party's public
storage, and replay the signatures, hashes, confirmations, and rules.
Verification needs only the public keys inside the chain, offline and with no
account.

## How does Mayfly use grant auth and JWS to enable verification?

When a user signs in to an app with Pubky Ring, their identity key signs a
Grant: a permission, in JWS form, that names the key the app will sign with.
Mayfly signs every record with that app key, also as a JWS, so a verifier
checks the Grant under the user's pubky, checks that the app key it names is
the one that signed the record, and checks the record itself, all offline.

## What happens if parties disagree?

A round that can no longer reach quorum ends, and the next round has a single
designated proposer chosen by rotation. Every vote is signed, so a refusal or
a double vote is attributable, and honest parties converge within two rounds.

## What if someone stops responding?

When every confirmation is required, one absent party holds the chain. The
parties still present can record that this person has left, and the rules
turn that into an outcome, such as a loss or an archived list; a watchman's
receipts are what prove the silence.

## What do provisional, final, and witnessed mean?

The newest link is provisional until the link after it carries its
confirmations, after which it is final. Witnessed `m/k` is separate: how many
of the `k` watchmen named for the chain have recorded a receipt for that
confirmation.

## Why use a watchman, and how?

The parties can show each other what they agreed and in which order, but not
when; a watchman is the impartial observer that timestamps each record and
makes a later rewrite visible. The chain's first record names it, it publishes
a signed engagement and a receipt for every record it sees, and a verifier
reads those receipts for questions of time while the chain itself advances on
the parties' votes.

## Does every chain need a watchman?

No. The chain advances on the parties' confirmations either way; without a
watchman, questions of time, and a copy that outlives the parties, rest on the
parties who are still there.

## How does Mayfly ensure witnessed data integrity?

Every committed link is mirrored to the other parties' homeservers, and each
record is signed and hash-linked, so any of those copies still verifies if
the author later alters or deletes the file on theirs. A watchman always
keeps a signed receipt of each record's hash; keeping the bytes themselves is
an optional `mirror` service, used when the chain needs a backup beyond the
parties' own copies.

## What happens when a key is replaced or lost?

A party moves to a new key with a `rekey` link, signed by the old key and
carrying the new Grant, which is how a device change is handled. If the key
is gone, `recover` is signed by the new key instead: it is never confirmed
automatically, a delay of at least a day applies, and the old key can veto it
while it is still the newest link.

## What can Mayfly not do?

It does not support large or open groups, private chains, hidden information,
or randomness beyond a simple commit-and-reveal in this version. It proves
what was agreed and when, not that the content was true, and it does not
settle payments.

## Can I build a Mayfly app?

Yes. An app supplies a versioned, deterministic rules function — current state
and a link in, next state out — and the Mayfly client proposes, confirms, and
verifies links on each party's Pubky storage. A shared list, chess, and an
agreed document are built this way; a new app is a new rules id, named in the
chain's first record, plus a page that drives the client.
