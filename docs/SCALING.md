# Scaling

How far the current implementation goes, what stops it, and what to change. Read from the
code as it stands (`crates/watchman`, `crates/client/src/pubky_store.rs`,
`crates/client/src/chain.rs`), with the numbers worked from it rather than measured under
load. Where a figure is an estimate it says so. Section references are to `docs/MAYFLY.md`.

## The short answer

A watchman watching tens of customers and a handful of live chains is well within a small
container: the process is a few megabytes and nearly idle, it signs one Ed25519 receipt per
new record, and it keeps at most eight HTTP requests in flight. Connections are not the
constraint. **Requests are.** Every sweep re-lists every watched folder and asks the
homeserver for a `HEAD` on every file in it, so the work per sweep grows with the total
number of files it can see, not with the number of new records. At the default 15-second
sweep that is comfortable for a few chains of modest length and becomes a problem in the
low hundreds of chains, or sooner with long chains or customers who keep a lot under `/pub/`.

The same pattern is in the party clients and the viewer. It is the one thing to fix before
Mayfly is used at any scale, and the fix is ordinary: list from a cursor, stop asking for
`HEAD`s on files that cannot change, and let the homeserver's event stream say when to look.

## What a sweep costs today

`Operator::sweep` (`crates/watchman/src/operator.rs`) does two things, one after the other,
on one task.

**Find work.** For every customer pubky it lists the whole of that customer's `/pub/` and
looks for `index/active/<chain>` markers. `PubkyStore::list` pages the listing 1,000 at a
time and then sends a `HEAD` for every entry, eight at a time, to learn each file's ETag.
The watchman does not use those ETags for markers. So a customer with 5,000 files in `/pub/`
(a social app's posts, say) costs about 5,000 requests per sweep before any chain is looked
at.

**Poll every chain.** For each engaged chain, `Watchman::collect` lists two prefixes in
every party's folder — `chains/<id>/` and `keys/` — and again `HEAD`s every entry. Only
files not seen before, or whose ETag changed, are fetched. Fetching is cheap; listing is not,
because it is proportional to everything already there.

How many files that is. Each committed link leaves one link file and `N − 1` confirmations,
and every party mirrors all of them (§7), so each party folder gains about `N` files per seq.
With a watchman engaged, every party also mirrors that watchman's receipts, roughly one per
record, so about `N · W` more per seq. Across `N` folders:

```
files per committed link  ≈  N² · (1 + W)
```

For two members and one watchman that is 8 files per link; for three, 18. A three-member
list of 100 items has around 1,800 files across its folders. One sweep of that chain is
about 1,800 `HEAD` requests plus six listings. Fifty such chains is 90,000 requests every
15 seconds, or 6,000 requests per second aimed at the members' homeservers — from one
watchman, doing nothing new.

Sweeps are sequential. One slow homeserver delays every chain behind it, and a sweep that
takes longer than the interval is simply followed by the next; there is no budget and no
concurrency across chains.

## Where the limits sit, in order

1. **Listings with a `HEAD` per file.** `PubkyStore::list` was written for the party
   client's tamper check on mirrored files (§7) and is reused everywhere. For the
   watchman's purposes the ETags are unnecessary: links, confirmations and receipts are
   named by hash and cannot change in place; only `rejects/` are overwritable, and
   `collect` already refetches those unconditionally. Cost today: `O(total files)` requests
   per sweep. Cost with the `HEAD`s removed: `O(folders)`.

2. **Listing `/pub/` in full to find markers.** The marker path is fixed
   (`/pub/<app>/mayfly/index/active/<id>`), but the app id is not known in advance, so the
   operator lists everything. The homeserver supports a `shallow` listing; one shallow list
   of `/pub/` names the apps, and one targeted listing per app finds the markers. Cost
   today: `O(customer's public files)` per customer per sweep. With shallow listing:
   `O(apps)`.

3. **No cursor.** Links are named `<seq8>-<h16>` and listings are lexicographic, so a chain
   folder can be listed from the last seen seq forward. Today every sweep starts from the
   top. With a cursor a quiet chain costs a handful of requests per sweep whatever its
   length.

4. **Polling where there is a stream.** The homeserver publishes an SSE event stream per
   user with a path filter, and the party clients already use it. The watchman does not. One
   stream per party homeserver (the server allows 50 users per stream) would tell the
   watchman which folder changed and reduce polling to a slow safety net.

5. **Sequential, unbounded sweeps.** One task, one chain at a time, no per-request or
   per-sweep deadline beyond the SDK's retries. A homeserver that hangs stalls the service;
   a sweep that overruns delays receipts for every chain, which is exactly the timing the
   watchman exists to give.

6. **No persistence of what was receipted.** `seen`, `receipted` and `votes` live in memory.
   On restart the watchman lists every folder, treats every record as new, and issues a
   second receipt for each, with a later `observed_at`. This was seen in the manual plan
   (I14: receipts 6 → 14 after a restart with one new item). The fold reads every receipt in
   the folder, so nothing breaks, but the timeline is padded, the restart cost is a full
   re-receipt of every engaged chain, and the folder grows with each restart. The keypair
   already lives on a volume; the receipted set belongs beside it, or can be rebuilt from
   the watchman's own `witness/<id>/` folder on start.

7. **Hard ceilings in the readers.** `MAX_FILES_PER_SWEEP` (watchman) and
   `Policy::max_files_per_sync` (client) are both 10,000 and truncate the listing. They are
   there so a hostile folder cannot make a reader read forever, but they are also a ceiling
   on honest chains: by the estimate above a three-member chain with one watchman passes
   10,000 files at roughly 550 links; a two-member chain at about 1,250. Past that point the
   watchman stops seeing new records and a client's verdict is built from a partial listing.
   The cursor in point 3 is also the fix here: the cap should bound files read *per sweep*,
   not files ever seen.

8. **Party clients do the same work.** `ChainClient::act` runs `sync` on every tick (three
   seconds in the list app) and on every event, and `sync` lists every known folder with the
   same `HEAD`-per-file listing. A member with a 100-item list on screen sends about 1,800
   requests every three seconds. With several members and the viewer polling every four
   seconds, a modest chain generates thousands of requests a minute against its members'
   homeservers. The fixes are the same three: no `HEAD` on hash-named files, a cursor, and
   trusting the event stream for the interval.

9. **Browser connections.** Each open chain holds one event stream per other member. A
   browser allows about six connections to one host, so a member of several open chains on
   the same homeserver, or the same app in several tabs, runs out. The client already opens
   no stream for the member's own folder and closes streams on leaving; a page that shows
   many chains at once needs one stream per homeserver, multiplexed, not one per chain.

10. **Homeserver quotas.** `pubky-homeserver` rate-limits by path, by IP and by user, and
    has bandwidth quotas per user and per anonymous IP (`config.sample.toml`,
    `[[drive.rate_limits]]`, `[default_quotas]`). A watchman's reads are anonymous, so an
    operator's `unauthenticated_ip_rate_read` applies to it. At the request volumes above a
    watchman would be throttled long before its own hardware mattered — and throttled reads
    look, to the fold, like a witness that missed records.

11. **Mirror tier storage.** At the `mirror` tier the watchman writes a copy of every record
    into its own account, so its homeserver storage quota grows with the sum of every chain
    it mirrors. Records are kilobytes; a thousand long chains is gigabytes. A watchman
    selling the mirror tier needs a quota to match, and a way to let mirrors of finished
    chains go when the engagement lapses.

12. **Memory.** About a kilobyte per tracked file (`seen`, and `receipted` holds the decoded
    receipt). A chain of 1,000 records is under a megabyte; a thousand such chains is under
    a gigabyte. `finished` and `markers` grow for the life of the process and are never
    pruned. Fine at hundreds of chains; worth a bound at thousands.

13. **Two witnesses adjudicate nothing through an outage.** Not a load limit, but a scaling
    choice apps will make: with `k = 2` any single outage leaves every time question
    *asserted* (§11.2). Three engaged witnesses adjudicate through one outage. A watchman
    that is down for a sweep or throttled for a minute is that outage.

## What the hardware needs to be

The process itself is small at every scale that matters: a few megabytes at rest, a few
hundred microseconds of CPU per new record (one signature check, one signature), and no
disk except the keypair. The sizing question is entirely about request rate and the
homeservers on the other end.

| Scale | Customers | Live chains | Today (per 15 s sweep) | After the fixes | Hardware |
| --- | --- | --- | --- | --- | --- |
| Hobby | 10 | 5 short lists | Low thousands of requests; fine | Tens | Any container: 1 shared vCPU, 128 MB |
| Small operator | 50 | 50 chains, ~100 links | ~100,000 requests; sweep overruns, members' homeservers throttle | A few hundred | 1 vCPU, 256 MB; the limit is the homeservers' quotas, not the box |
| Service | 1,000 | 1,000 chains, mixed | Not viable as written | ~10,000 spread over SSE and slow polls; ~10 Hz steady | 2 vCPU, 1–2 GB for the tracked-file maps; persistence on a volume; one stream per party homeserver |

"After the fixes" means points 1–4 and 6 above. Without them, the honest sizing is: one
watchman per small group of chains, and a sweep interval chosen so that the requests per
sweep stay well under what the members' homeservers allow an anonymous IP.

## Other load risks in the protocol

- **Chain length is a first-class cost.** Every reader replays every record on every sync
  (§9.2), and every mirror copies every record. The spec's answer is that chains are short
  and content is small (§13); a long-lived list or a document with thousands of edits is a
  chain that will one day cross the 10,000-file ceiling and then be read partially. Apps
  should close and start again at a sensible length, and the readers should raise or remove
  the ceiling once they list from a cursor.

- **Mirroring multiplies writes by `N`.** Each committed link is written `N` times, and
  each receipt `N` times more. This is by design (§7, §12) and is what makes evidence
  survive a homeserver; it is also why the file estimate above is quadratic in `N`. Groups
  of eight, the spec's upper suggestion, write 64 files per link before receipts.

- **Bodies are part of every folder.** A 60 KB contract redline is mirrored `N` times and,
  at the mirror tier, once more. `max_body_bytes` (default 65,536) bounds each; nothing
  bounds the sum but chain length.

- **Discovery is polling.** Both the watchman (markers) and clients (`sync` over declared
  folders) find out about change by asking. The homeserver has a push channel; the protocol
  does not depend on it, and should not, but the implementations should use it as the
  primary trigger and poll as the fallback.

- **Everything above scales the homeservers, not Mayfly.** Members' homeservers carry the
  read load of every party, every watchman and every viewer of every chain they hold. A
  popular chain with many viewers is a popular set of files on a few homeservers. That is
  Pubky's problem to solve with caching and quotas; Mayfly's part is to stop asking the same
  questions every few seconds.

## What to do, in order

1. Split `PubkyStore::list` into a listing without `HEAD`s and a tamper check on demand;
   use the former in the watchman everywhere and in the client for hash-named files.
2. List `chains/<id>/links/` (and confirms, rejects, receipts) from a cursor at the last
   seen seq; make `MAX_FILES_PER_SWEEP` and `max_files_per_sync` bound one sweep, not the
   chain.
3. Shallow-list `/pub/` for app folders, then list `index/` under each.
4. Rebuild `receipted` from the watchman's own `witness/<id>/` on start (or persist it
   beside the keypair), so a restart issues no second receipts.
5. Subscribe the watchman to one event stream per party homeserver; poll on a slow interval
   as the fallback. Same for the client's `wait_for_event`, which already exists.
6. Poll chains concurrently with a small limit, a per-request deadline, and a per-sweep
   budget; report an overrun on `/status`.
7. Then measure: a load harness against the compose stack with `n` synthetic chains of
   length `L` and `N` members, recording requests per sweep and sweep duration. The numbers
   above are derived from the code; that harness would replace them with observed ones.
