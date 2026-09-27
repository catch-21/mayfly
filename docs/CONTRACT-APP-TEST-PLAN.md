# Contract app test plan

A manual session for the contract app. Nothing here is automated. Each case says what to do
and what must be true afterwards. Record pass, fail, or blocked, and paste the chain URL on
any failure. A re-run overwrites the result for that case and appends a note to the session
log. Score against the written expectation.

The app under test is the testnet flavour. Two browser profiles, because one origin keeps a
single sign-in. Unanimity is the only quorum the app creates. `contract/1` is two parties and
one open offer: the party who did not write it may accept, reject outright, or revise.

## Setup

1. From `mayfly/`, `docker compose up` so a homeserver is on the testnet ports. A watchman is
   only needed for the cases that name one.
2. `npm run build:testnet` in `apps/contract`. Serve member A on port 4173 and member B on
   4174, each from `dist/testnet/`, so the page is `/testnet/`.
3. Sign each member in with **New testnet identity**. Copy the pubky from the home screen.
   Reloading the same origin must restore that identity.
4. Watchman pubky, for the cases that use one: `GET http://127.0.0.1:8790/status` and read
   `pubky`. Recreate the watchman with `MAYFLY_WATCHMAN_FREE` set to both member pubkys.
5. Hard-refresh a tab that was open before the rebuild.

## How to score

On the contract page, after each confirmed step, both members show the same text. A step
that is not yet confirmed is a pending row, not yet the open offer or the outcome. "Something
to look at" stays empty unless the case asks for an anomaly. A member who has not joined is
marked not joined. An empty seat list is not that test: the absent member's chip says they
have not joined.

## A. Sign-in and home

| Id | Steps | Expected |
| --- | --- | --- |
| A1 | Open the app on a fresh origin. | Sign-in shows the Pubky Ring QR and the testnet shortcut. |
| A2 | Shortcut, blank token. | Home shows your pubky. "Your contracts" is empty. |
| A3 | Reload the same origin. | The same pubky, still signed in. |
| A4 | Create with the other-party field empty. | Refused in the page. No chain is written. |
| A5 | Create with your own pubky as the other party. | Treated as empty. Refused, same as A4. |
| A6 | Create with two other pubkys. | Refused: a contract names exactly one other party. No chain is written. |
| A7 | Create with a string that is not a pubky. | The page says it is not a pubky. No chain is written. |
| A8 | Open `https://example.com/not-a-contract` from the link field. | Refused. You stay on the home screen. |

## B. Two parties, no watchman

Leave the watchman field empty.

| Id | Steps | Expected |
| --- | --- | --- |
| B1 | A creates a contract naming B. | A sees "Waiting for the other party", no text box, and B marked not joined. |
| B2 | B opens the invite. | B sees the two parties, unanimity, `contract/1`, and no watchman, and a Join button. The app does not join by itself. |
| B3 | B joins. | Both see an empty contract and a box to write the first offer. |
| B4 | A proposes a short text. | Until B's app confirms it, it is pending. Then both show it as the open offer, with no redline. A is waiting. B sees Accept, Reject outright, and a revision box filled with the text. |
| B5 | B sends a revision that changes one line. | Both show a redline: the old line removed, the new line added. A now has the three answers. B is waiting. |
| B6 | A accepts. | Both show the agreed text. Each can record that they agreed. The note says refusing that close is obstruction. |
| B7 | A records the agreement. B agrees. | The chain is finished, outcome `agreed`. Home lists it once, marked finished. Reopening shows the agreed text and no answer buttons. |
| B8 | A new contract. A offers. B rejects outright. Both record that there is no contract. | Outcome `rejected`. The rejected text is still shown. Home marks the chain finished. |
| B9 | A new contract. A offers. A proposes to end with no contract. B refuses. | The offer is still open. Nothing under "Something to look at". B can still accept, reject, or revise. |
| B11 | After B9, the party asked to put that close forward again clicks **Let it go**. | Both show "No contract" and the last text. The same question does not appear for the other party. Each can record the ending. After both record it, the chain is finished, outcome `no contract`, and reopening has no answer card. |
| B12 | After an accept (B6), refuse the finished close, then on the follow-up card click **Record the ending** and have the other agree. | The chain finishes as `agreed`. There is no **Let it go** on that card, and the question does not rotate. |
| B10 | A proposes a text longer than 65,536 bytes. | The page says how big the record was and that the chain allows 65,536. The contract is unchanged. |

## C. A bystander

| Id | Steps | Expected |
| --- | --- | --- |
| C1 | A third identity opens B7's invite. | "Not your contract". The agreed text is shown. The page is not offering to join, accept, or close. |
| C2 | Leave that tab open across B4 on a different chain, then open a finished invite. | A finished chain stops being polled: reloading still shows the text, and the page says it is final. |

## D. With a watchman

Name the watchman pubky at create. Both members are free customers.

| Id | Steps | Expected |
| --- | --- | --- |
| D1 | Create, join, and accept an offer as in B2–B7. | The join screen names one watchman. After the finished close, both still show the agreed text. The watchman's pubky is unchanged if it is restarted; a new receipt still verifies. |

## Session log

25 Sep 2026, testnet, watchman `qp5do5x5dnjmy1pxyrbat5k88pihk3uube33f9fdcef5ocrumtho`.
Two parties and a bystander, each in its own browser profile. The watchman was a free
customer of both parties.

Pass: A1–A8, the watched happy path (create, consent naming one watchman, join, offer,
redline revision, accept, finished close as `agreed`, ten receipts), C1, outright reject,
refused walk-away, let-it-go then finished close as `no contract`, refused finished close
then record-the-ending (no Let it go on that card), oversize (`95537` bytes against
`65536`, contract unchanged). After a watchman restart the pubky was unchanged and an open
contract stayed under watch. The finished happy chain left the active watch list, which is
where a finished chain belongs.
