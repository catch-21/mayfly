# List app test plan

A manual session for the list app, the chain viewer, and the watchman together. Nothing here
is automated. Each case says what to do and what must be true afterwards. Record pass, fail,
or blocked, and paste the chain URL on any failure.

The app under test is the testnet flavour. Two browser profiles, or the same app served on
two origins, because one origin keeps a single sign-in. A third origin is the viewer.
Three-member cases need a third origin.

Unanimity is the only quorum the app creates: a change is confirmed only when every member
has voted. `list/1` lets any seated member add, edit the text of an item, tick, untick, remove
or archive. An edit keeps the item's id, its place in the list, and whether it is ticked.
Quantity is a rule the app does not offer. An ordinary append does not skip a silent member;
silence only moves the chain when a round has already died and a designated proposer has
been quiet.

## Setup

1. From `mayfly/`, bring up the current compose file so the service is `watchman` and the
   variables are `MAYFLY_WATCHMAN_*`. A container left over from before the rename will not
   do. `docker compose down` first if one is still running.
2. Serve the testnet builds. Member A on port 4173, member B on 4174, member C on 4176,
   the viewer on 4175, each from that app's `dist/` so the page is `/testnet/`.
   Rebuild `apps/list` and `apps/view` with `npm run build:testnet` before the session.
3. Sign each member in with **New testnet identity**, not Pubky Ring. Copy the pubky from
   the home screen. Reloading the same origin must restore that identity; a new shortcut
   on a fresh origin is a new person.
4. Watchman pubky: `GET http://127.0.0.1:8790/status` and read `pubky`. For the engaged
   cases, recreate the watchman with `MAYFLY_WATCHMAN_FREE` set to every member pubky in
   that case, then confirm `/status` lists them as free. For the not-engaged case, leave
   `MAYFLY_WATCHMAN_FREE` empty.
5. Hard-refresh a tab that was open before the rebuild.

Open the viewer on a chain by putting the invite URL in the hash:
`http://127.0.0.1:4175/testnet/#` plus the URL-encoded invite.

## How to score a chain

On the list page, after each confirmed change:

- Every member who has the app open shows the same items, in the same order, with the same
  text and the same ticks.
- A change that is not yet confirmed is shown as pending, with a vote count, and is not yet
  a ticked or removed row.
- The status line counts confirmed changes, not pending ones. Genesis is not one of them.
- "Something to look at" stays empty. Equivocation, a tampered mirror, or a rules refusal
  showing up on a case that does not ask for it is a fail.

On the viewer, for the same invite URL:

- Chain id, rules `list/1`, and the same parties.
- One timeline row per committed link, genesis first. Each later row's body is the add,
  tick, untick, remove, archive or close just made.
- Seats for every member who joined. A member who has not joined is absent.
- No red file row (bad signature, bytes against the ETag, bytes against the name) unless the
  case asks for one. A confirmation is named by the link it votes for, so that must not be
  reported as a name mismatch.
- While the chain is not final the page says it is live. After a successful close it stops
  and says the chain is final.

## A. Sign-in and home

| Id | Steps | Expected |
| --- | --- | --- |
| A1 | Open the app on a fresh origin. | Sign-in shows the Pubky Ring QR and, because this is the testnet build, the shortcut. |
| A2 | Shortcut, blank token. | Home shows your pubky. "Your lists" is empty, not an error. |
| A3 | Reload the same origin. | The same pubky, still signed in. |
| A4 | Create with the members field empty. | Refused in the page. No chain is written. |
| A5 | Create with your own pubky typed into the members field, and nobody else. | Treated as empty. Refused, same as A4. |
| A6 | Create with a string that is not a pubky, plus one real member. | The create fails in the page and says why. No half-written list on the home screen. |
| A7 | Open `https://example.com/not-a-list` from the link field. | Refused. You stay on the home screen. |
| A8 | Sign out, then sign in again with the shortcut on that same origin. | A different pubky. The previous lists are not yours. |

## B. Two members, no watchman

Leave the watchman field empty. Members A and B.

| Id | Steps | Expected |
| --- | --- | --- |
| B1 | A creates the list naming B. | A sees "Waiting for the others", both pubkys, and an invite link. The add box is not offered. B is marked not joined. |
| B2 | B opens the invite. | B sees the join screen: both members, unanimity, no watchman. B is not yet seated. |
| B3 | Someone who is neither A nor B opens the same invite. | "Not your list." No join button. |
| B4 | B joins. | Both apps show an empty list and both members joined. Viewer: genesis committed, two seats, no witnesses, no red rows. |
| B5 | B adds "Milk". | Until A has confirmed, both apps show it pending, not as a row. Then both show Milk unticked, one confirmed change. Viewer timeline has genesis and one add whose body text is Milk. |
| B6 | A adds "Eggs" while B's app is open. | B confirms it without a prompt. Both lists are Milk then Eggs. |
| B7 | B ticks Milk. | A confirms. Both show Milk ticked and Eggs unticked. |
| B8 | A unticks Milk. | B confirms. Both show Milk unticked. |
| B9 | A removes Eggs. | B confirms. Eggs is gone. Milk remains. Viewer body for that link is a remove, and the rules checklist matches the list page. |
| B10 | B adds "Bread", and before A confirms, A also adds "Butter". | Both proposals show as pending. The chain does not end up with both as rows from that round: one round dies, one item is carried forward and confirmed, the other is not. Neither app shows an anomaly. Record which item survived. |
| B11 | The member whose item lost in B10 adds it again. | It confirms and appears. The list now has the survivor from B10 and this one. |
| B12 | A ticks Milk, and before B confirms, B ticks Milk too. | One tick confirms. Milk is ticked once. No duplicate row, no anomaly. |
| B13 | Add an item of only spaces. | The Add button stays disabled, or the page refuses it. No blank row. |
| B14 | Add a 500-character item. | It confirms. Both apps and the viewer show the full text. |
| B15 | Reload both origins on this list's URL. | Same identity, same items, no second join. |
| B16 | A edits Milk to "Oat milk". | Until B confirms, Milk is still the row and the edit is pending. Then both show Oat milk, unticked, in the same place. Viewer body for that link is an edit of Milk's id, text "Oat milk". |
| B17 | B edits Oat milk to "Semi-skimmed". | A confirms. Both show Semi-skimmed. The id is unchanged. |
| B18 | A opens an edit and clears the text, or types only spaces. | The control stays disabled, or the page refuses it. The item's text is unchanged. |
| B19 | A edits Semi-skimmed to "Full cream", and before B confirms, B edits the same item to "Skimmed". | Both edits show as pending. One text survives that round and is confirmed; the other does not. No anomaly. Record which text survived. The survivor's id is still Semi-skimmed's id. |

## C. Two members, finishing

Continue from B, or a fresh two-member list with one confirmed item.

| Id | Steps | Expected |
| --- | --- | --- |
| C1 | A chooses Archive. | B confirms it with no decision card (it is an ordinary append). Both show the list archived: no add box, and edit, tick and remove disabled. The item is still listed, with its text. |
| C2 | On that archived list, try to add and try to edit. | Neither control is offered. If a proposal is forced, the page shows an error and the list is unchanged. |
| C3 | Fresh list, one confirmed item. A chooses Propose to close. | B gets a decision card, not a silent confirm. The item is still there and the chain is not final. |
| C4 | B refuses the close. | The card goes away. The list is still usable. A further add confirms. Viewer does not show a final chain. |
| C5 | A proposes to close again. B agrees. | Both show the list closed. Add, tick and remove are gone. Viewer: status final, live polling stopped, a close link in the timeline. |
| C6 | After C5, B opens the invite again. | The closed list, not a join screen. |

## D. Three members, no watchman

Members A, B and C. Watchman field empty.

| Id | Steps | Expected |
| --- | --- | --- |
| D1 | A creates naming B and C. Only B joins. | The list does not become usable. A still shows C as not joined. No item can be confirmed. |
| D2 | C joins. | All three show an empty list, all three joined. Viewer: three seats. |
| D3 | A adds "Milk". Only B's app is open. | The row stays pending at 2 of 3. It must not appear as a confirmed item. |
| D4 | C opens the app. | C confirms without a prompt. All three show Milk. |
| D5 | B adds "Eggs" and C adds "Bread" before anyone confirms. | Same shape as B10, but the survivor still needs the third member. All three end on the same single new item. No anomaly. |
| D6 | A proposes to close. Only B agrees. | Not final. C still has the decision card. |
| D7 | C agrees. | Final for all three. Viewer final, three confirmers on the close. |

## E. Two members, watchman engaged

Set `MAYFLY_WATCHMAN_FREE` to A and B's pubkys and recreate the watchman. Create the list with that pubky in the watchman field.

| Id | Steps | Expected |
| --- | --- | --- |
| E1 | A creates. B reads the join screen before joining. | The join screen names one watchman. |
| E2 | B joins. Wait for a sweep (about 5 seconds) past the `index/active` marker. | `/status` shows this chain under `watching`, receipts at least 1, errors 0. Both list pages mention a watchman. Viewer: one witness, an engagement file, signature ok, not red. |
| E3 | B adds "Milk". A confirms. | Both show Milk. The status line reports the head witnessed. Viewer: the add row's witnessed count is not `0/0`, and a receipt file for it decodes with `signature_ok`. |
| E4 | A removes Milk. B confirms. | Milk is gone on both. Viewer receipt count has moved on. `/status` errors still 0. |
| E5 | Restart only the watchman container. | `/status` reports the same pubky as before the restart. The chain is still listed once the next sweep runs. The list page does not gain an anomaly from the restart. |

## F. Watchman named, not engaged

`MAYFLY_WATCHMAN_FREE` empty. Create a two-member list that still names the watchman pubky.

| Id | Steps | Expected |
| --- | --- | --- |
| F1 | B joins, A and B add and confirm one item. | The list works as in B. Viewer witnesses stay "none engaged". Witnessed on the add is `0/0` or absent. `/status` watching does not include this chain. No anomaly: a dark watchman is not a fault in the list. |

## G. Three members, watchman engaged

`MAYFLY_WATCHMAN_FREE` is A, B and C. Watchman field set.

| Id | Steps | Expected |
| --- | --- | --- |
| G1 | All three join, then A adds "Milk" and the other two confirm. | All three show Milk. Viewer: three seats, one witness, the add witnessed. |
| G2 | C closes their app. B ticks Milk. | Pending at 2 of 3 until C returns. Opening C's app confirms it. No skip, no anomaly. |
| G3 | A proposes to close and B agrees. C has the app closed, then opens it and agrees. | Final only after C agrees. Viewer final, witness still shown, no red rows. |

## H. Viewer on its own

Use the chain from B after B9, and the chain from E after E4.

| Id | Steps | Expected |
| --- | --- | --- |
| H1 | Paste the invite URL. | Timeline, parties, seats, files. Live indicator while the chain is open. |
| H2 | Paste a link to one record in that chain (a `links/0000000N-<h16>.jws` URL from the files panel) instead of the invite. | The same chain, not a single-file view and not an error. |
| H3 | Paste the invite with no trailing slash, and again with a trailing slash. | Both open the chain. |
| H4 | While H1 is open, a member adds an item and the other confirms. | Within about 4 seconds the viewer shows the new row without a manual refresh. |
| H5 | Open the closed chain from C5. | Status final. The live line is gone. Refresh does not start polling again. |
| H6 | Paste `pubky://aaaa/not-a-chain`. | A readable error. No blank page. |

## I. After the fixes

Not part of the first session. Each of these is `unchecked` in the session log. Run them once
the failures above are addressed. They are the boundaries the first session touched only by
accident, or did not touch, including what the viewer shows for a chain that is finished and
for one that is only part-way there.

| Id | Steps | Expected |
| --- | --- | --- |
| I1 | Create with your own pubky and one other member in the members field. | Your pubky is not a second seat. The other person is the only invitee. A normal two-member list. |
| I2 | Create with the same other pubky typed twice. | Refused in the page. No chain on Your lists. |
| I3 | Create with one valid pubky that has spaces around it and a trailing comma. | Treated as that one member. The list is created. |
| I4 | A and C already have the three-member list open. B joins. | Without a reload, A and C show B joined and an empty list. They agree with B and with the viewer. |
| I5 | Add an item. If the page says the list is settling, wait a few seconds and add it again. | The item confirms. The form does not stay disabled, and it does not fail with no error. |
| I6 | Close a list, then look at Your lists. | That chain appears once, marked finished. It is not also listed as open. |
| I7 | On a fresh two-member list with one item, propose to close, and have the other member refuse. Then add another item. | No Obstruction on either app or in the viewer. The new item confirms. The chain is not final. |
| I8 | Tick an item, then edit its text. | The new text, still ticked, same place. The viewer edit names the same id. |
| I9 | Add "Milk", confirm it, then add "Milk" again. | Two rows, both unticked. Both confirm. |
| I10 | Add an item whose text is `Café, 2×`. | Both apps and the viewer show that text exactly. |
| I11 | Add an item longer than the chain allows (genesis `max_body_bytes` is 65536). | The page refuses it. The list is unchanged. No anomaly. |
| I12 | Two members, watchman field empty, while `MAYFLY_WATCHMAN_FREE` includes both of them. Add and confirm one item. | `/status` watching does not include this chain. The list has no watchman line and no anomaly. |
| I13 | Continue from F, where the watchman was named but had no free credit. Set `MAYFLY_WATCHMAN_FREE` to both members and recreate the watchman. | A later sweep engages that chain. Receipts move. Neither app shows an anomaly. The viewer shows one witness. |
| I14 | On a chain the watchman is already watching, restart the watchman. In the viewer, open a receipt from before the restart and one written after it. | Both signatures are ok. The pubky is unchanged. The list page gains no anomaly. |
| I15 | Start Create, switch away from that tab for about a minute, then come back. | The list exists, or the page shows an error. It is not still "Creating…" with no error. |
| I16 | B adds an item, and before it confirms A proposes to close. | One of the two survives that round. Both apps agree on which. No Obstruction. If the add survives, it is a normal row and the chain is not final. If the close survives, the other member still has to agree, and the chain is not final until they do. |
| I17 | With one confirmed item, B removes it and A confirms. Then A adds a new item. | The list is empty in between, and the add box stays. The new item then confirms. |
| I18 | B adds an item, and before it confirms A archives. | Both apps end on the same list. Either the add is in the archived list, or the archive won and the item is absent. No anomaly. |
| I19 | A creates a two-member list. Before B joins, paste the invite into the viewer. | The viewer shows the chain. It is not an error and it does not stay on "Fetching the chain…". Status is not final and the live line is present. The creator's genesis file is listed and decodes. B is not a seat. No red file row. |
| I20 | Both members have joined and one item is confirmed. B adds a second item. Open the viewer before A confirms. | Status is not final and the live line is present. The checklist shows the first item and not the second. No red file row. When A confirms, the second item appears within about 4 seconds, without a manual refresh. |
| I21 | Close a two-member list by agreement, with no earlier refuse. Paste that invite into the viewer. | Status final. Polling has stopped. The timeline ends with a close. The checklist matches the list page. No anomaly. Refresh does not start polling again. |

## Session log

Update this table in place. A re-run overwrites that row's result and adds a short note; do not
delete earlier notes. Results are `pass`, `fail`, `blocked`, `unchecked`, or `not run`.
`unchecked` means the case is in the plan and has not been tried. Paste the chain URL on any
failure.

Environment: 23 Sep 2026. Compose recreated (`watchman`, empty `MAYFLY_WATCHMAN_FREE`).
Homeserver `http://127.0.0.1:6286` returned 200. Watchman `/healthz` was `ok`. Apps rebuilt
with `build:testnet` and served at 4173 (A), 4174 (B), 4176 (C), viewer 4175.
Member A before A8: `3mak4zm1h5f8kzbbqmh5zjdsk6m1qg631xbbt67zgpbgesdcoiry`.
Member A for B onwards: `ohxc9q6kgnrfyw1nhfb5so1mygbjssrqzfeyimxtohwms999foso`.
Member B: `pka7amqf48jnuoootdyuthxapynr7f8pm415x16psd6ejc13ua6y`.
Stranger, and member C for later sections: `6dg5mnoi8du4ibxar7wqm8fnthoqz9hsrggxxbtyrpyggtj8ru7o`.

| Id | Result | Notes |
| --- | --- | --- |
| A1 | pass | Sign-in showed the Pubky Ring link and New testnet identity. |
| A2 | pass | Home showed the pubky. Your lists: none yet. |
| A3 | pass | Reload kept the same pubky and the empty list. |
| A4 | pass | Empty members: "name at least one other member by pubky". Stayed on home. |
| A5 | pass | First session: own pubky was not treated as empty (`genesis invalid: fewer than two parties`). Recheck 23 Sep, separate Chrome profiles: refused as empty, no list written. |
| A6 | pass | First session wrote `pubky://3mak4zm1…/chains/8MS0ND8DKB5PA07RDPC99BE4ER/` for `not-a-pubky`. Recheck: refused with "is not a pubky" before any chain was written. |
| A7 | pass | `https://example.com/not-a-list` refused. Stayed on home. |
| A8 | pass | New pubky `ohxc9q6kgnrfyw1nhfb5so1mygbjssrqzfeyimxtohwms999foso`. Previous list was not on that home. This identity is member A for section B. |
| B1 | pass | First session (`…/chains/QFEHDVW0DXCYGHXP74KFTGMR64/`): add box and UnconfirmedProposal before anyone joined. Recheck: waiting screen, no add box, no anomaly. `pubky://i9cu58ihjxxx9o6pzmfzpkwhkmqc9uzoi9fnngucmckn84k5mmdy/pub/list.mayfly.example/mayfly/chains/7YQWSV42H01EAB1K7J4366NP64/`. |
| B2 | pass | B saw Join, both members, unanimity, no watchman, and was not seated. |
| B3 | pass | Stranger `6dg5mnoi8du4ibxar7wqm8fnthoqz9hsrggxxbtyrpyggtj8ru7o` saw "Not your list" and no Join button. |
| B4 | pass | Both apps showed an empty list and both members joined. Viewer: genesis committed, two seats, witnesses none engaged, no red file rows. |
| B5 | pass | Both settled on Milk unticked, one confirmed change. Viewer add body text is Milk. The pending line was gone within 2s, so it was not sampled. |
| B6 | pass | B confirmed Eggs with no decision card. Both lists: Milk then Eggs. |
| B7 | pass | Both: Milk ticked, Eggs unticked. |
| B8 | pass | Both: Milk unticked. |
| B9 | pass | Eggs gone, Milk remains. Viewer link is a remove; checklist is Milk. |
| B10 | pass | Butter survived and Bread did not. No anomaly. Both-pending was not on screen at the first read, about 3s later. |
| B11 | pass | B added Bread. Both lists: Milk, Butter, Bread. |
| B12 | pass | Milk ticked once. One new confirmed change. No duplicate row, no anomaly. |
| B13 | pass | Spaces left Add disabled. Submitting the form did not add a row. |
| B14 | pass | 500-character item confirmed. Both apps and the viewer show all 500 characters. |
| B15 | pass | Reload kept ohxc9q… on A and pka7am… on B, the same items, and no join screen. |
| B16 | pass | First session: no edit control. Recheck: A edited Milk to Oat milk. B confirmed. Still unticked. |
| B17 | pass | B edited Oat milk to Semi-skimmed. A confirmed. Text replaced in place. |
| B18 | pass | A blank edit left Save disabled. The text stayed Semi-skimmed. |
| B19 | pass | Both edited the same item before either confirmed. Full cream survived. No anomaly. |
| C1 | pass | Archived the B chain. No decision card. Add, tick and remove disabled. Items kept their text. There is no edit control to disable. |
| C2 | pass | On that archived list the add box is absent, and there is no edit control. |
| C3 | pass | First session (`…/chains/3W4DBQ43MW6SVKNGG1QVCGQFMR/`): decision card plus UnconfirmedProposal. Recheck: decision card, Apples stayed, no "Something to look at". `pubky://i9cu58ihjxxx9o6pzmfzpkwhkmqc9uzoi9fnngucmckn84k5mmdy/pub/list.mayfly.example/mayfly/chains/1PEDXWYYT2XF8RXQBN4SPQ9RY0/`. |
| C4 | pass | First session: Refuse came back as Obstruction. Recheck: the card cleared, no Obstruction, and Pears then confirmed. |
| C5 | pass | First session: closed, but the viewer kept Obstruction from the refused close, and the home screen listed the chain twice. Recheck: both apps closed. Viewer final, polling stopped, a close in the timeline, no Obstruction. Home listed that chain once, marked finished. |
| C6 | pass | First session: closed list, plus Obstruction. Recheck: B reopened the invite and saw the closed list, not Join, and no Obstruction. |
| D1 | pass | First session (`…/chains/BYGWH0VPKH2S80WGB6DG5Z944G/`): add box and a vanished add while C had not joined. Recheck: after only B joined, A still waited, C marked not joined, no add box. `pubky://i9cu58ihjxxx9o6pzmfzpkwhkmqc9uzoi9fnngucmckn84k5mmdy/pub/list.mayfly.example/mayfly/chains/C7VZ7JNX6KJKRJZ5GEZHQE03E8/`. |
| D2 | pass | First session: Join sat on "Joining…" for over 20 seconds. Recheck, separate profiles: C joined in 810ms, B in 1608ms. All three then showed an empty list. |
| D3 | pass | First session: Add failed with "settling a change" and Milk never appeared. Recheck: Milk confirmed by all three, no settling error. |
| D4 | unchecked | Not part of the recheck. The D3 blocker is gone. |
| D5 | unchecked | Not part of the recheck. |
| D6 | unchecked | Not part of the recheck. |
| D7 | unchecked | Not part of the recheck. |
| E1 | pass | First attempt stayed on "Creating…" in a background tab; a fresh page then created `…/chains/Q3242Y4FYNWYYRJR43M48HM11R/`. Recheck, profile in the foreground: create naming the watchman returned in 415ms. `pubky://i9cu58ihjxxx9o6pzmfzpkwhkmqc9uzoi9fnngucmckn84k5mmdy/pub/list.mayfly.example/mayfly/chains/Q4MAKRM8WKWVQH0553P1B1NQX4/`. |
| E2 | pass | After B joined, `/status` watched that chain, receipts 2, errors 0. Both pages mention a watchman. Viewer: one witness, `engage.jws` signature ok, no red row. |
| E3 | pass | Both showed Milk. Status witnessed 1/1. Viewer add row witnessed 1/1, receipt `00000001-…` signature ok. |
| E4 | pass | Milk gone on both. Receipts moved from 4 to 6. Errors 0. |
| E5 | pass | After restart, `/status` pubky unchanged and the chain still watched. Neither list gained an anomaly. |
| F1 | pass | Chain `pubky://ohxc9q6kgnrfyw1nhfb5so1mygbjssrqzfeyimxtohwms999foso/pub/list.mayfly.example/mayfly/chains/WG7SNGX0G3QTWXANDRXFM3FBZ0/`. Rice confirmed, no anomaly. Viewer: none engaged, add witnessed 0/0. `/status` watching is empty. |
| G1 | pass | First session (`…/chains/YDQPD1X8FWE2YPXMBT4VMQEDAC/`): A and C kept showing B as not joined until a reload, then Add hung and a reload stayed on "Loading…". Recheck, separate profiles: all three converged without a reload and Milk confirmed. `pubky://i9cu58ihjxxx9o6pzmfzpkwhkmqc9uzoi9fnngucmckn84k5mmdy/pub/list.mayfly.example/mayfly/chains/AT0EXCGZNXJJV0BBTG82S08RJM/`. |
| G2 | unchecked | Not part of the recheck. The G1 blocker is gone. |
| G3 | unchecked | Not part of the recheck. |
| H1 | pass | B chain after B15: timeline, parties, seats, files, live indicator. |
| H2 | pass | `links/00000001-38WX39QM115Q9RD5.jws` opened the same chain. No error. |
| H3 | pass | Invite with and without a trailing slash both opened the chain. |
| H4 | pass | Tea was added and confirmed. The viewer checklist showed it without a manual refresh. |
| H5 | pass | Closed chain from C5: status final, polling stopped, and refresh did not start it again. The Obstruction row from C4 is still on that page. |
| H6 | pass | `pubky://aaaa/not-a-chain` showed `is not a chain URL`. The page was not blank. |
| I1 | unchecked | |
| I2 | unchecked | |
| I3 | unchecked | |
| I4 | unchecked | |
| I5 | unchecked | |
| I6 | unchecked | |
| I7 | unchecked | |
| I8 | unchecked | |
| I9 | unchecked | |
| I10 | unchecked | |
| I11 | unchecked | |
| I12 | unchecked | |
| I13 | unchecked | |
| I14 | unchecked | |
| I15 | unchecked | |
| I16 | unchecked | |
| I17 | unchecked | |
| I18 | unchecked | |
| I19 | unchecked | Viewer, chain not yet joined by everyone. |
| I20 | unchecked | Viewer, one change confirmed and another still open. |
| I21 | unchecked | Viewer, chain closed by agreement. |

## Failures

Collected from the first session. Rechecked on 23 Sep 2026 with one Chrome profile per
member (and a fourth for the viewer), against the fixes below. Every item passed. The
session log rows carry both the first result and the recheck. G2, G3 and D4–D7 were not
part of the recheck; they are unchecked now that their blockers are gone.

1. **A5.** Typing only your own pubky is not treated as an empty members field. The page shows `genesis invalid: fewer than two parties`.
2. **A6.** A members field containing `not-a-pubky` still writes a chain, and that chain stays on Your lists. Chain `pubky://3mak4zm1h5f8kzbbqmh5zjdsk6m1qg631xbbt67zgpbgesdcoiry/pub/list.mayfly.example/mayfly/chains/8MS0ND8DKB5PA07RDPC99BE4ER/`.
3. **B1, D1.** Before every member has joined, the creator is shown the add box and `UnconfirmedProposal at change 0`, not "Waiting for the others". The pending line reads `n/0`.
4. **B16–B19.** There is no control to edit an item's text, so those cases could not be run.
5. **C3.** A pending close shows a decision card and also `UnconfirmedProposal`. Chain `pubky://ohxc9q6kgnrfyw1nhfb5so1mygbjssrqzfeyimxtohwms999foso/pub/list.mayfly.example/mayfly/chains/3W4DBQ43MW6SVKNGG1QVCGQFMR/`.
6. **C4, C5, C6.** Refusing a close is recorded as `Obstruction`. The decision card comes back. The closed chain stays final, and the obstruction is still shown in the list and the viewer.
7. **C5, home screen.** That closed list is listed twice: once as an open list and once as finished.
8. **D1.** An add attempted before the third member joined disappeared with no error.
9. **D2.** The third member's Join stayed on "Joining…" for well over 20 seconds. The seat did appear later.
10. **D3.** After everyone had joined, Add failed with "The list is settling a change; try again in a moment" and Milk was never proposed.
11. **E1, first attempt.** Creating a list that names the watchman stayed on "Creating…" with no error while that tab sat in the background. A fresh page created the list, so this may be a stalled tab rather than a refused create. The retry is E1 pass.
12. **G1.** On `pubky://ohxc9q6kgnrfyw1nhfb5so1mygbjssrqzfeyimxtohwms999foso/pub/list.mayfly.example/mayfly/chains/YDQPD1X8FWE2YPXMBT4VMQEDAC/` the viewer seated all three and one watchman, but A and C kept showing B as not joined until a reload. A's Add of Milk then never returned: the form stayed disabled, no item was written, and a reload stayed on "Loading…". G2 and G3 were not run.

### What was changed for them

Recorded here so the recheck knew what it was looking at. The session log now has both the
first result and the recheck.

- **1 (A5).** `Home.tsx` drops the creator's own pubky from the members field, so a field
  containing only that pubky is empty and is refused before anything is written. Recheck:
  refused as empty, no list written.
- **2 (A6).** The same parser refuses anything that is not a z32 pubky before a chain is
  written, and the core's genesis safety check now refuses a party or witness that is not a
  pubky, so no client can write such a genesis. Recheck: `not-a-pubky` refused, no chain
  written.
- **3 (B1).** The list page treats a chain whose genesis is not yet committed as waiting,
  whoever has signed: the creator sees "Waiting for the others" and no add box.
  `UnconfirmedProposal` attributed to nobody is no longer listed under "Something to look at".
  Recheck: waiting screen, no add box, no anomaly.
- **4 (B16–B19).** Each item has an edit control (✎): inline text, Save on Enter, Cancel on
  Escape, blank or unchanged text refused. It sends `list/1`'s `edit`, keeping id, place and
  tick. Disabled once archived or closed, like tick and remove. Recheck: Oat milk, then
  Semi-skimmed, a blank edit refused, and a contested edit settled on Full cream with no
  anomaly.
- **5 (C3).** `UnconfirmedProposal` attributed to nobody is the pending state the page already
  shows, so it is no longer listed under "Something to look at". The viewer still shows it.
  Recheck: the decision card appeared and Apples stayed, with nothing under "Something to look
  at".
- **6 (C4–C6).** Protocol change, recorded in the spec's obstruction paragraph: refusing a
  `close` with reason `agreed` is consent withheld, and a verifier does not count it as
  obstruction. Covered by `refusing_an_agreed_close_is_not_obstruction` in the core tests.
  The decision card now clears as soon as it is answered, and is dropped once I have voted in
  the round or the round has died. The proposer, when the round dies and it is their turn, is
  asked "put it forward again, or let it go" rather than being shown their own proposal to
  agree with. Recheck: Refuse cleared the card and left no Obstruction, a later add confirmed,
  and the agreed close was final in the viewer and on reopening, with no Obstruction.
- **7 (home screen).** `mark_finished` removes the active marker whether or not this client
  wrote it, and "Your lists" shows one row per chain, reading finished first. Recheck: the
  closed chain appeared once, marked finished.
- **8 (D1).** Same waiting screen as B1, so an add is not offered until every member has
  joined. Recheck: after only B joined, A still waited, C was marked not joined, and there was
  no add box.
- **9 (D2).** The likely cause of the long "Joining…" was connection exhaustion: each live
  event stream holds an HTTP connection to the homeserver, browsers allow about six per host,
  and several list tabs plus the viewer against one local homeserver were at that limit.
  Streams that were still being opened when the seat set changed were never closed. The page
  now opens no stream for its own folder and closes a stream that finished opening after the
  seat set changed. Recheck, separate Chrome profiles: C joined in 810ms.
- **10 (D3).** Same connection changes as D2, and a user action with no answer in twenty
  seconds releases the form and says so. The stalled call itself cannot be cancelled; the
  message says to reload if it persists. Recheck: Milk confirmed by all three, no settling
  error.
- **11 (E1).** Same connection changes and the twenty-second timeout. Recheck, in the
  foreground: create naming the watchman returned in 415ms.
- **12 (G1).** Same connection changes, and the list page now treats an uncommitted genesis as
  waiting for everyone who has signed, so a member who has joined early is not shown as still
  missing. Recheck, separate profiles: all three converged without a reload and Milk confirmed.

## Not in this session

- Pubky Ring sign-in, and a homeserver that requires a signup token. The shortcut covers the testnet only.
- Quantity. The rules allow it on an add or an edit; the app does not send it.
- A member going offline long enough for a round to rotate. `list/1` obliges nobody, so an ordinary tick does not start a skip. D5 and G2 cover silence on an open vote.
- Mainnet, and two different homeservers. Every identity here is on the compose testnet.
