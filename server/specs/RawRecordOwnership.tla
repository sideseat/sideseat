------------------------ MODULE RawRecordOwnership ------------------------
(***************************************************************************)
(* The life of one raw record - the stored export a set of span rows is     *)
(* derived from - under redelivery, deletion, retention, legal hold, backup *)
(* and restore, with its ownership arbitrated in the transactional store.   *)
(*                                                                         *)
(* This is the contract the implementation is being brought to. Today's     *)
(* reconciliation still decides who names a record from the analytics       *)
(* store's own rows, which on ClickHouse is the read-then-delete race this  *)
(* protocol replaces; the ownership registry, the epoch, and the collection *)
(* decisions below are the design for that replacement, not a description   *)
(* of code that already exists.                                             *)
(*                                                                         *)
(* One export carries the spans `Spans`. Its record is stored as versions;  *)
(* readers take the latest by (epoch, version, insertion order), which is   *)
(* what the stored number `(epoch << 32) | version` gives DuckDB's          *)
(* append-only table and ClickHouse's ReplacingMergeTree(version) alike.    *)
(*                                                                         *)
(* Who owns the record is decided in the transactional store, never by      *)
(* looking at the rows in the analytics store:                              *)
(*                                                                         *)
(*   - an ingest registers its staged payload as a pending writer; the      *)
(*     registration is the staged row, created once, and settlement retires *)
(*     it forever, by whichever execution settles it;                       *)
(*   - once its rows commit, their spans own the record durably - but the   *)
(*     confirm statement grants ownership only when the version it would    *)
(*     rest on is numbered above every collection decision, pending or      *)
(*     completed. The decisions and their high-water mark live in the same  *)
(*     store as the grant, so the fence is one transaction: a granted       *)
(*     owner's version is above every decided epoch, and no purge - which   *)
(*     is always bounded by a decided epoch - can ever reach it. An         *)
(*     execution refused by the fence re-keys its version above the         *)
(*     high-water mark and confirms again;                                  *)
(*   - a deletion takes ownership away in the transaction that journals it, *)
(*     the retention sweep in the one that removes the rows;                *)
(*   - a collection is one transactional decision: no owner, no pending     *)
(*     writer, no staged rewrite, no hold. It records the collection at the *)
(*     project's epoch and bumps the epoch; the purge then deletes the      *)
(*     versions numbered at or below it, and readers ignore them until the  *)
(*     decision is dropped.                                                 *)
(*                                                                         *)
(* Deleted content is removed in two steps. A rewrite - the latest record   *)
(* without the deleted spans - is staged and written as a redelivery is,    *)
(* registered and released with its payload; then a supersede purge deletes *)
(* the versions below the latest, which hold nothing the latest does not   *)
(* but deleted content.                                                    *)
(*                                                                         *)
(* Deletion and retention are not alike. A deletion is journalled, and the  *)
(* journal is permanent while the store lives: a deleted span is never      *)
(* owned again, and the confirm statement excludes it. (A restore of the    *)
(* embedded stores rolls the journal back to the backup with everything     *)
(* else - the documented residual of `backup-restore.mdx`.) Retention is a  *)
(* predicate - the span's data age against the project's current setting -  *)
(* so it moves both ways: time and a shortened setting expire a span, a     *)
(* lengthened setting un-expires it. Expiry therefore takes ownership only  *)
(* in the sweep that physically removes the rows, never at confirm time: a  *)
(* span re-delivered while expired is owned, invisible behind the retention *)
(* predicate every reader applies, and swept - or kept, if retention grew   *)
(* meanwhile.                                                               *)
(*                                                                         *)
(* An execution of a staged payload can outlive the payload's settlement:   *)
(* redrive re-runs a payload still being written, the slower execution      *)
(* lands its writes after the registration was retired, and it can die at   *)
(* any step (`IDie`) with nothing left to re-run it - settlement deleted    *)
(* the staged copy. On ClickHouse (AtomicWrites FALSE) nothing can stop a   *)
(* stale write landing - it read the epoch before the decision, and the     *)
(* insert is conditional on nothing in another table - so it can land below *)
(* the latest, or bring a collected record back. Such a write is harmless:  *)
(* the confirm fence never grants ownership on it, no reader gated by live  *)
(* ownership sees it, and it can never exist unqueued - the durable queue   *)
(* row is written before every append - so the pass that consumes its entry *)
(* collects or rewrites whatever the writer, alive or dead, left behind.    *)
(* On DuckDB (AtomicWrites TRUE) the look and the write are one             *)
(* transaction, but the epoch is read from the transactional store before   *)
(* it, so the stale-epoch landing exists there too and the same fence       *)
(* covers it.                                                               *)
(*                                                                         *)
(* Checked instances (distinct states; wall time on a loaded 10-core        *)
(* M-series laptop, two checks sharing it):                                  *)
(*   RawRecordOwnership.cfg     1,417,568    37s                             *)
(*   RawRecordTakeover.cfg     10,631,599  4m19s                             *)
(*   RawRecordReconcilers.cfg  15,589,306  8m03s                             *)
(*   RawRecordRestore.cfg       2,622,898    16s                             *)
(* Mutations, on RawRecordTakeover.cfg's constants: Epochs = FALSE breaks    *)
(* NoLatestLost; the fence removed from Confirm (grant always) breaks       *)
(* Covered; NeverResurrected added to INVARIANTS is violated in seconds, so  *)
(* the stale landing the reclaim machinery is about is reachable.           *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Spans, Ingests, Rewriters, Reconcilers, MaxVer,
          WithHold,          \* explore legal holds
          WithRestore,       \* explore backup and restore
          ReceiptPrecedence, \* a row's instant is its export's receipt, not its write
          WithCrash,         \* a reconciler, or an execution past settlement, can die between steps
          AtomicWrites,      \* the look and the write it decides are one step (DuckDB)
          Duplicates,        \* the ingests are executions of one staged payload, not separate deliveries
          Epochs             \* versions carry the project epoch, and a collection's purge is bounded by it

ASSUME Spans # {} /\ Ingests # {} /\ Reconcilers # {} /\ MaxVer \in Nat
ASSUME WithHold \in BOOLEAN /\ WithRestore \in BOOLEAN /\ ReceiptPrecedence \in BOOLEAN
ASSUME WithCrash \in BOOLEAN /\ AtomicWrites \in BOOLEAN /\ Duplicates \in BOOLEAN /\ Epochs \in BOOLEAN

VARIABLES
    rows,      \* spans that currently have a row naming the record
    store,     \* every stored version: [ep, ver, seq, content]
    seq,       \* insertion order, the tie-break between equal versions
    tomb,      \* spans removed by a deletion: journalled, shrinks only when a restore rolls the journal back
    expired,   \* spans currently below the project's retention: a predicate, so a longer setting shrinks it
    hold,      \* a project legal hold is active
    heldOnce,  \* one hold per behaviour: a second adds states, not interleavings
    unexOnce,  \* one retention lengthening per behaviour, for the same reason
    frozen,    \* the latest content when the hold began, and what was deleted by then (history)
    queued,    \* an unclaimed reconciliation entry for the record exists: entries dedup per record
    ipc, ikept, iwritten, isnap,
    wpc, wsnap,
    rpc, rrec, rhad, rclaim, rdec,
    backup,    \* one snapshot of both stores, when `taken`
    later,     \* spans a revision received after this export has been written for
    overtaken, \* ghost: a row of this export was written over a revision received after it
    aown,      \* the arbiter's durable owners: spans whose committed rows name the record
    apend,     \* the arbiter's pending writers: staged payloads registered and not yet settled
    aret,      \* payloads settled: their registration is gone and no execution recreates it
    rwpend,    \* a rewrite payload is staged and not yet settled
    rwcontent, \* what the staged rewrite holds
    rwid,      \* which staging of a rewrite payload is the current one
    aepoch,    \* the project epoch: one transactional sequence, bumped by every collection decision
    acoll,     \* collections decided and not yet dropped: the epochs they were decided at
    everColl,  \* every epoch a collection was decided at; its maximum is the stored high-water mark
    latestLost \* ghost: a purge deleted the latest version of a record its rows own

vars == <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued,
          ipc, ikept, iwritten, isnap, wpc, wsnap, rpc, rrec, rhad, rclaim, rdec,
          backup, later, overtaken, aown, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl,
          latestLost>>

Version == [ep : Nat, ver : 0..MaxVer + 2, seq : Nat, content : SUBSET Spans]

\* The token of an ingest's staged payload: shared by its executions when they are duplicates of one payload.
Token(i) == IF Duplicates THEN CHOOSE j \in Ingests : TRUE ELSE i

\* A key is (epoch, version, insertion order), compared in that order.
Below(a, b) == \/ a.ep < b.ep
               \/ a.ep = b.ep /\ a.ver < b.ver
               \/ a.ep = b.ep /\ a.ver = b.ver /\ a.seq < b.seq

\* A version at or below any collection decision - pending or completed - is gone as far as any reader is
\* concerned, forever: the reader's test is one comparison against the project's collected high-water mark, so
\* dropping a decision can never un-hide bytes a straggling purge is still entitled to delete. The confirm
\* fence keeps every owned version above the mark, so no wanted version is ever hidden. Without epochs there is
\* nothing to tell an old incarnation from a new one.
Live(S, coll) == {v \in S : ~(Epochs /\ \E e \in coll : v.ep <= e)}
LiveStore == Live(store, acoll \cup everColl)

Present == LiveStore # {}
LatestOf(S) == CHOOSE r \in S : \A o \in S : o = r \/ Below(o, r)
Latest == LatestOf(LiveStore)
LatestContent == IF Present THEN Latest.content ELSE {}
NoVersion == [ep |-> 0, ver |-> 0, seq |-> 0, content |-> {}]
LatestOr == IF Present THEN Latest ELSE NoVersion

\* A finished or dead process keeps no snapshot: every field below is freshly written before it is next read,
\* and resetting it collapses states that differ only in a dead process's memory.
ZeroSnap == [present |-> FALSE, latest |-> NoVersion, ep |-> 0, vep |-> 0]

\* A key above `v`, in an epoch no earlier than `e`: the next version of the same epoch, or the first of a later one.
Above(v, e, step) == IF v.ep >= e THEN [ep |-> v.ep, ver |-> v.ver + step] ELSE [ep |-> e, ver |-> 1]

\* An epoch at or below a collection decision, pending or completed: the version it keys is one some purge is
\* entitled to delete, however late that purge runs. The implementation's test is one comparison against the
\* decisions table and the stored high-water mark, in the transactional store.
Doomed(ep) == Epochs /\ \E e \in acoll \cup everColl : ep <= e

Append(key, content) ==
    /\ store' = store \cup {[ep |-> key.ep, ver |-> key.ver, seq |-> seq, content |-> content]}
    /\ seq' = seq + 1

\* Idempotent: one unclaimed entry per record - the insert is a no-op while one exists, which is both the
\* byte-cheap implementation and what makes claims precise: a clear deletes only the entry its read claimed.
Enqueue == queued' = TRUE

TypeOK ==
    /\ rows \subseteq Spans
    /\ store \subseteq Version
    /\ tomb \subseteq Spans
    /\ hold \in BOOLEAN
    /\ queued \in BOOLEAN
    /\ rclaim \in [Reconcilers -> BOOLEAN]
    /\ ipc \in [Ingests -> {"fence", "register", "insert", "look", "land", "write", "confirm", "rekey",
                            "check", "repair", "settle", "done"}]
    /\ wpc \in [Rewriters -> {"idle", "append", "settle"}]
    /\ rpc \in [Reconcilers -> {"idle", "act", "purge", "drop", "recheck", "clear"}]
    /\ aown \subseteq Spans /\ apend \subseteq Ingests /\ aret \subseteq Ingests /\ acoll \subseteq Nat

Init ==
    /\ rows = {}
    /\ store = {}
    /\ seq = 0
    /\ tomb = {}
    /\ expired = {}
    /\ hold = FALSE
    /\ heldOnce = FALSE
    /\ unexOnce = FALSE
    /\ frozen = [content |-> {}, tomb |-> {}]
    /\ queued = FALSE
    /\ ipc = [i \in Ingests |-> "fence"]
    /\ ikept = [i \in Ingests |-> {}]
    /\ iwritten = [i \in Ingests |-> {}]
    /\ isnap = [i \in Ingests |-> ZeroSnap]
    /\ wpc = [w \in Rewriters |-> "idle"]
    /\ wsnap = [w \in Rewriters |-> [key |-> [ep |-> 0, ver |-> 0], content |-> {}, id |-> 0]]
    /\ rpc = [r \in Reconcilers |-> "idle"]
    /\ rrec = [r \in Reconcilers |-> NoVersion]
    /\ rhad = [r \in Reconcilers |-> FALSE]
    /\ rclaim = [r \in Reconcilers |-> FALSE]
    /\ rdec = [r \in Reconcilers |-> 0]
    /\ backup = [taken |-> FALSE, state |-> <<>>]
    /\ later = {}
    /\ overtaken = FALSE
    /\ aown = {}
    /\ apend = {}
    /\ aret = {}
    /\ rwpend = FALSE
    /\ rwcontent = {}
    /\ rwid = 0
    /\ aepoch = 0
    /\ acoll = {}
    /\ everColl = {}
    /\ latestLost = FALSE

ivars == <<ipc, ikept, iwritten, isnap>>
wvars == <<wpc, wsnap>>
rvars == <<rpc, rrec, rhad, rclaim, rdec>>
avars == <<aown, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl>>

----------------------------------------------------------------------------
(* Ingest: the deletion fences decide what is kept, the payload registers,  *)
(* the record is inserted if absent, the rows are written, the confirm      *)
(* fence grants ownership - or sends the execution to re-key first - and    *)
(* the record is checked against the rows before the payload settles.       *)

\* A fence that keeps nothing settles the payload: the delivery is deliberately absent, and staging retires
\* deliberately absent payloads rather than leaving a registration that would block collection forever.
Fence(i) ==
    /\ ipc[i] = "fence"
    /\ ikept' = [ikept EXCEPT ![i] = Spans \ tomb]
    /\ iwritten' = [iwritten EXCEPT ![i] = {}]
    /\ isnap' = [isnap EXCEPT ![i] = ZeroSnap]
    /\ IF Spans \ tomb = {}
         THEN /\ apend' = apend \ {Token(i)}
              /\ aret' = aret \cup {Token(i)}
              /\ ipc' = [ipc EXCEPT ![i] = "done"]
         ELSE /\ ipc' = [ipc EXCEPT ![i] = "register"]
              /\ UNCHANGED <<apend, aret>>
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued,
                   wvars, rvars, backup, later, overtaken, aown, rwpend, rwcontent, rwid, aepoch, acoll,
                   everColl, latestLost>>

\* Never refused: a collection decided before it stays decided, and the confirm fence keeps this payload's
\* versions above it. Registering a token another execution of the payload already holds changes nothing - the
\* registration is the staged row, created once. An execution of a payload already settled registers nothing:
\* the row is gone, the execution runs on unregistered, and everything it writes is healed.
Register(i) ==
    /\ ipc[i] = "register"
    /\ apend' = apend \cup ({Token(i)} \ aret)
    /\ ipc' = [ipc EXCEPT ![i] = "insert"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ikept,
                   iwritten, isnap, wvars, rvars, backup, later, overtaken, aown, aret, rwpend, rwcontent, rwid,
                   aepoch, acoll, everColl, latestLost>>

\* The epoch is read from the transactional store before any analytics write, on either backend, so the number
\* a version lands under can be stale by the time it lands. The confirm fence is what makes that safe.
Insert(i) ==
    /\ ipc[i] = "insert"
    /\ isnap' = [isnap EXCEPT ![i].ep = aepoch]
    /\ ipc' = [ipc EXCEPT ![i] = "look"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ikept,
                   iwritten, wvars, rvars, backup, later, overtaken, avars, latestLost>>

\* Insert-if-absent. With AtomicWrites the look and the write are one analytics transaction; without it the
\* look is taken first and the write lands later on its strength (Land). `vep` is the epoch of the version this
\* execution's ownership would rest on: the one it wrote, or the latest it observed.
\*
\* Every version lands with a queue entry: in one transaction with it where the backend has one (DuckDB), and
\* written before it where it does not (ClickHouse: the look enqueues, the delayed write carries nothing). An
\* entry whose write never lands settles trivially; an entry consumed before the delayed write lands leaves the
\* landing coverable only by the maintenance scan, which finds it through its version key. A pass that collects
\* a record out from under a writer still confirming is safe: the confirm fence refuses the doomed version and
\* the writer re-keys above the decision.
Look(i) ==
    /\ ipc[i] = "look"
    /\ IF AtomicWrites
         THEN /\ IF Present
                   THEN /\ isnap' = [isnap EXCEPT ![i].vep = Latest.ep]
                        /\ UNCHANGED <<store, seq, queued>>
                   ELSE /\ \E v \in {1, MaxVer} :
                             Append([ep |-> IF Epochs THEN isnap[i].ep ELSE 0, ver |-> v], ikept[i])
                        /\ isnap' = [isnap EXCEPT ![i].vep = IF Epochs THEN isnap[i].ep ELSE 0]
                        /\ Enqueue
              /\ ipc' = [ipc EXCEPT ![i] = "write"]
         ELSE /\ isnap' = [isnap EXCEPT ![i].present = Present, ![i].latest = LatestOr]
              /\ IF Present THEN UNCHANGED queued ELSE Enqueue
              /\ UNCHANGED <<store, seq>>
              /\ ipc' = [ipc EXCEPT ![i] = "land"]
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

\* The entry for this delayed write was enqueued at the look, before it - a version can never land without an
\* entry having existed first - and the write itself carries none: on this backend the two tables share no
\* transaction. The write also carries a landing fence the backend itself enforces: the insert selects nothing
\* past its deadline (bounding when the query can start, on the backend's clock the deadline was taken from at
\* the look) and runs under the backend's execution-time cap (bounding start to commit; the row is one block,
\* so a kill commits nothing). However long the request floats - past a client timeout, a disconnect, or the
\* process's death - it cannot physically land after deadline + cap, which is the horizon the entry's deferred
\* clear waits out, plus the replica clock-skew margin.
Land(i) ==
    /\ ipc[i] = "land"
    /\ IF isnap[i].present
         THEN /\ isnap' = [isnap EXCEPT ![i].vep = isnap[i].latest.ep]
              /\ UNCHANGED <<store, seq>>
         ELSE /\ \E v \in {1, MaxVer} :
                   Append([ep |-> IF Epochs THEN isnap[i].ep ELSE 0, ver |-> v], ikept[i])
              /\ isnap' = [isnap EXCEPT ![i].vep = IF Epochs THEN isnap[i].ep ELSE 0]
    /\ UNCHANGED queued
    /\ ipc' = [ipc EXCEPT ![i] = "write"]
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

Overtakes(spans) == ~ReceiptPrecedence /\ spans \cap later # {}

\* The rows land, and the batch's commit carries a queue entry for the record - rows, like versions, can never
\* exist unqueued, so what a writer that dies right here left behind is found by the pass.
Write(i) ==
    /\ ipc[i] = "write"
    /\ rows' = rows \cup ikept[i]
    /\ overtaken' = (overtaken \/ Overtakes(ikept[i]))
    /\ iwritten' = [iwritten EXCEPT ![i] = ikept[i]]
    /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "confirm"]
    /\ UNCHANGED <<store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, isnap,
                   wvars, rvars, backup, later, avars, latestLost>>

\* The span write fails, whether or not its rows landed - the caller cannot tell. What landed owns the record,
\* through the same fence a confirm passes; the payload keeps its registration and is delivered again from the
\* fences.
Fail(i) ==
    /\ ipc[i] = "write"
    /\ \E landed \in {{}, ikept[i]} :
        /\ rows' = rows \cup landed
        /\ overtaken' = (overtaken \/ Overtakes(landed))
        /\ aown' = aown \cup (IF Doomed(isnap[i].vep) THEN {} ELSE landed \ tomb)
    /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "fence"]
    /\ UNCHANGED <<store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten, isnap,
                   wvars, rvars, backup, later, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl,
                   latestLost>>

\* The rows committed: their spans own the record, durably - except those a deletion took meanwhile, which the
\* statement leaves out against the deletion journal it shares a store with. An expired span is owned all the
\* same: expiry is a predicate that may flip back, so only the sweep that removes the rows takes ownership. (A
\* span owned again after its rows were swept and retention grew is the raw record outliving a cache eviction:
\* the row is re-derivable from the record, and the span re-expires when its age passes the deadline again.)
\*
\* The fence: no grant on a doomed version. The decisions and the high-water mark are rows beside the owners,
\* so the test and the grant are one transaction, and a granted owner's version is above every epoch any purge
\* is bounded by. A refused execution re-keys first.
Confirm(i) ==
    /\ ipc[i] = "confirm"
    /\ IF Doomed(isnap[i].vep)
         THEN /\ ipc' = [ipc EXCEPT ![i] = "rekey"]
              /\ UNCHANGED aown
         ELSE /\ aown' = aown \cup (iwritten[i] \ tomb)
              /\ ipc' = [ipc EXCEPT ![i] = "check"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ikept,
                   iwritten, isnap, wvars, rvars, backup, later, overtaken, apend, aret, rwpend, rwcontent,
                   rwid, aepoch, acoll, everColl, latestLost>>

\* The refused execution writes its content again above the high-water mark it was refused against, queues the
\* record, and confirms anew. The look inside this write is as stale as any (the epoch safety never rests on
\* it); the fresh epoch comes from the refusing transaction, which a later decision can only exceed.
Rekey(i) ==
    /\ ipc[i] = "rekey"
    /\ LET key == Above(LatestOr, IF Epochs THEN aepoch ELSE 0, 2) IN
         /\ key.ver <= MaxVer + 2
         /\ Append(key, LatestContent \cup ikept[i])
         /\ isnap' = [isnap EXCEPT ![i].vep = key.ep]
    /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "confirm"]
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

\* The check, after the rows own the record: no collection can be decided from here on - the spans are owned -
\* so a repair it writes stays. The look is taken first; with AtomicWrites the repair is written in the same
\* analytics transaction.
\*
\* The order is load-bearing: rows, ownership, this check, then the payload settles and the export is
\* acknowledged.
Check(i) ==
    /\ ipc[i] = "check"
    /\ isnap' = [isnap EXCEPT ![i].present = Present, ![i].latest = LatestOr, ![i].ep = aepoch]
    /\ IF Present /\ iwritten[i] \subseteq Latest.content
         THEN /\ ipc' = [ipc EXCEPT ![i] = "settle"]
              /\ IF iwritten[i] \cap tomb # {} \/ \E v \in LiveStore : Below(v, Latest)
                   THEN Enqueue ELSE UNCHANGED queued
              /\ UNCHANGED <<store, seq>>
         ELSE IF AtomicWrites
                THEN /\ Above(LatestOr, aepoch, 2).ver <= MaxVer + 2
                     /\ Append(Above(LatestOr, IF Epochs THEN aepoch ELSE 0, 2), LatestContent \cup ikept[i])
                     /\ Enqueue
                     /\ ipc' = [ipc EXCEPT ![i] = "settle"]
                ELSE /\ ipc' = [ipc EXCEPT ![i] = "repair"]
                     /\ UNCHANGED <<store, seq, queued>>
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

\* Without AtomicWrites, the repair lands on the strength of the look: above the latest it saw, in the epoch it
\* read, whatever was written since. Its epoch safety comes from ownership - the spans are owned, so no new
\* collection can be decided, and the epoch the check read already exceeds every old one. Queued, as every
\* write is.
Repair(i) ==
    /\ ipc[i] = "repair"
    /\ LET key == Above(isnap[i].latest, IF Epochs THEN isnap[i].ep ELSE 0, 2) IN
         /\ key.ver <= MaxVer + 2
         /\ Append(key, isnap[i].latest.content \cup ikept[i])
    /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "settle"]
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ikept, iwritten, isnap,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

\* The payload settles; its registration is retired and never recreated - which deletes the staged copy, so an
\* execution that dies after this point has nothing left to re-run it.
Settle(i) ==
    /\ ipc[i] = "settle"
    /\ apend' = apend \ {Token(i)}
    /\ aret' = aret \cup {Token(i)}
    /\ ipc' = [ipc EXCEPT ![i] = "done"]
    /\ ikept' = [ikept EXCEPT ![i] = {}]
    /\ iwritten' = [iwritten EXCEPT ![i] = {}]
    /\ isnap' = [isnap EXCEPT ![i] = ZeroSnap]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued,
                   wvars, rvars, backup, later, overtaken, aown, rwpend, rwcontent, rwid,
                   aepoch, acoll, everColl, latestLost>>

\* An execution whose payload another execution settled dies mid-flight: its process is gone, the staged copy
\* was deleted at settlement, and nothing re-runs it. (An execution whose payload is still pending needs no
\* death action: `Fail` already returns it to the fences, which is what redrive does.) Death does not cancel a
\* write the process already submitted; both of that write's outcomes are explored - the step fires and the
\* death follows it, or the landing fence voids the request and the step never fires.
IDie(i) ==
    /\ WithCrash
    /\ Token(i) \in aret
    /\ ipc[i] \notin {"fence", "done"}
    /\ ipc' = [ipc EXCEPT ![i] = "done"]
    /\ ikept' = [ikept EXCEPT ![i] = {}]
    /\ iwritten' = [iwritten EXCEPT ![i] = {}]
    /\ isnap' = [isnap EXCEPT ![i] = ZeroSnap]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued,
                   wvars, rvars, backup, later, overtaken, avars, latestLost>>

----------------------------------------------------------------------------
(* The rewrite: the latest record without the deleted spans, staged by a    *)
(* reconciler and written by any execution of the staged payload - several, *)
(* when redrive re-runs it. Its version numbers above the latest it read.   *)

RewriteLook(w) ==
    /\ wpc[w] = "idle"
    /\ rwpend
    /\ wsnap' = [wsnap EXCEPT ![w] = [key |-> Above(LatestOr, IF Epochs THEN aepoch ELSE 0, 1),
                                      content |-> rwcontent, id |-> rwid]]
    /\ wpc' = [wpc EXCEPT ![w] = "append"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars, rvars,
                   backup, later, overtaken, avars, latestLost>>

\* Lands on the strength of the look - with AtomicWrites in the same step as a fresh one. Queued.
RewriteAppend(w) ==
    /\ wpc[w] = "append"
    /\ LET key == IF AtomicWrites THEN Above(LatestOr, IF Epochs THEN aepoch ELSE 0, 1) ELSE wsnap[w].key IN
         /\ key.ver <= MaxVer + 2
         /\ Append(key, wsnap[w].content)
    /\ Enqueue
    /\ wpc' = [wpc EXCEPT ![w] = "settle"]
    /\ UNCHANGED <<rows, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wsnap, rvars,
                   backup, later, overtaken, avars, latestLost>>

\* Settles the staging it read - a later staging of another rewrite stays pending.
RewriteSettle(w) ==
    /\ wpc[w] = "settle"
    /\ rwpend' = (rwpend /\ rwid # wsnap[w].id)
    /\ wpc' = [wpc EXCEPT ![w] = "idle"]
    /\ wsnap' = [wsnap EXCEPT ![w] = [key |-> [ep |-> 0, ver |-> 0], content |-> {}, id |-> 0]]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   rvars, backup, later, overtaken, aown, apend, aret, rwcontent, rwid, aepoch, acoll,
                   everColl, latestLost>>

----------------------------------------------------------------------------
(* Deletion, the tombstone sweep, retention and holds.                     *)

\* Tombstone and journal, take its ownership away and enqueue the record, then delete the rows.
Delete(s) ==
    /\ ~hold
    /\ s \notin tomb
    /\ tomb' = tomb \cup {s}
    /\ rows' = rows \ {s}
    /\ later' = later \ {s}
    /\ aown' = aown \ {s}
    /\ Enqueue
    /\ UNCHANGED <<store, seq, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars, rvars, backup,
                   overtaken, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl, latestLost>>

\* The compensating re-check: rows written for a deleted span go again.
Sweep ==
    /\ rows \cap tomb # {}
    /\ rows' = rows \ tomb
    /\ Enqueue
    /\ UNCHANGED <<store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars, rvars,
                   backup, later, overtaken, avars, latestLost>>

\* Time passes, or the project's retention is shortened: the span falls below the deadline. Nothing is removed
\* yet - expiry is a predicate on the span's data age, which readers and the sweep apply.
ExpireMark(s) ==
    /\ s \notin expired
    /\ expired' = expired \cup {s}
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, unexOnce, frozen, queued, ivars, wvars, rvars,
                   backup, later, overtaken, avars, latestLost>>

\* The project's retention is lengthened: some spans are within it again. Rows the sweep already removed stay
\* removed; rows it had not reached are simply retained, and their spans still - or again - own the record.
Unexpire ==
    /\ ~unexOnce
    /\ unexOnce' = TRUE
    /\ \E back \in (SUBSET expired) \ {{}} : expired' = expired \ back
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, queued, ivars, wvars, rvars, backup,
                   later, overtaken, avars, latestLost>>

\* The retention pass: physically delete an expired span's rows and take its ownership, in the pass's own
\* transaction, and enqueue the record. It re-fires for rows a late execution wrote again while expired.
ExpireSweep ==
    /\ ~hold
    /\ \E s \in expired \cap (rows \cup aown) :
        /\ rows' = rows \ {s}
        /\ later' = later \ {s}
        /\ aown' = aown \ {s}
    /\ Enqueue
    /\ UNCHANGED <<store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars, rvars,
                   backup, overtaken, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl, latestLost>>

\* The maintenance scan: a version at or below the high-water mark is findable by an indexed predicate - the
\* version key itself is the durable debt record, needing no queue entry, claim or decision row - and the scan
\* queues its record on a timer. This is what reclaims what a dead writer left after its queue entry was
\* already consumed.
MaintSweep ==
    /\ \E v \in store : Doomed(v.ep)
    /\ Enqueue
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars,
                   rvars, backup, later, overtaken, avars, latestLost>>

SetHold ==
    /\ WithHold
    /\ ~hold
    /\ ~heldOnce
    /\ hold' = TRUE
    /\ heldOnce' = TRUE
    /\ frozen' = [content |-> LatestContent, tomb |-> tomb \cup expired]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, unexOnce, queued, ivars, wvars, rvars, backup, later,
                   overtaken, avars, latestLost>>

ReleaseHold ==
    /\ hold
    /\ hold' = FALSE
    /\ frozen' = [content |-> {}, tomb |-> {}]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, heldOnce, unexOnce, queued, ivars, wvars, rvars,
                   backup, later, overtaken, avars, latestLost>>

\* Another export, received after this one, writes its revision of a span: the span's winner is no longer ours.
\* With ReceiptPrecedence the revision's instant is its receipt whatever this export writes, so `later` cannot
\* be observed and exploring it would only multiply states.
Supersede(s) ==
    /\ ~ReceiptPrecedence
    /\ s \notin tomb
    /\ s \notin later
    /\ later' = later \cup {s}
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   wvars, rvars, backup, overtaken, avars, latestLost>>

----------------------------------------------------------------------------
(* Backup and restore of the embedded stores, taken with the server stopped *)
(* (`backup-restore.mdx`), so no ingest, rewrite or reconciler is part-way  *)
(* through; SQLite goes with DuckDB, so a payload staged at the backup is   *)
(* staged again after the restore, and the arbiter - owners, registrations, *)
(* epoch, decisions, and the deletion journal - with it.                    *)

Quiescent ==
    /\ \A i \in Ingests : ipc[i] \in {"fence", "done"}
    /\ \A w \in Rewriters : wpc[w] = "idle"
    /\ \A r \in Reconcilers : rpc[r] = "idle"

Backup ==
    /\ WithRestore
    /\ Quiescent
    /\ ~backup.taken
    /\ backup' = [taken |-> TRUE,
                  state |-> <<rows, store, aown, apend, rwpend, rwcontent, aepoch, acoll,
                              {i \in Ingests : ipc[i] = "fence" /\ ikept[i] # {}}, rwid, aret, tomb,
                              everColl>>]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   wvars, rvars, later, overtaken, avars, latestLost>>

\* Both stores roll back to the one snapshot - including the deletion journal, so a deletion made after the
\* backup is undone with everything else: the documented residual of `backup-restore.mdx`, which the operator
\* accepts by restoring. Expiry needs no restoring - it is recomputed from the timestamps the restored data
\* holds. A restore can take a later revision's rows back with it; `later` keeps them all the same, which only
\* matters without ReceiptPrecedence.
Restore ==
    /\ backup.taken
    /\ Quiescent
    /\ rows' = backup.state[1]
    /\ store' = backup.state[2]
    /\ aown' = backup.state[3]
    /\ apend' = backup.state[4]
    /\ rwpend' = backup.state[5]
    /\ rwcontent' = backup.state[6]
    /\ aepoch' = backup.state[7]
    /\ acoll' = backup.state[8]
    /\ ipc' = [i \in Ingests |-> IF i \in backup.state[9] THEN "fence" ELSE ipc[i]]
    /\ rwid' = backup.state[10]
    /\ aret' = backup.state[11]
    /\ tomb' = backup.state[12]
    /\ everColl' = backup.state[13]
    /\ Enqueue
    /\ rclaim' = [r \in Reconcilers |-> FALSE]
    /\ backup' = [taken |-> FALSE, state |-> <<>>]
    \* An operator's restore under a hold rebases what the hold protects: content created after the backup is
    \* destroyed by the restore itself, as `backup-restore.mdx` warns, and the hold now protects what came
    \* back. This is the one way held content changes while a hold is active.
    /\ frozen' = IF hold
                   THEN LET live == Live(backup.state[2], backup.state[8] \cup backup.state[13]) IN
                          [content |-> IF live # {} THEN LatestOf(live).content ELSE {},
                           tomb |-> backup.state[12] \cup expired]
                   ELSE frozen
    /\ UNCHANGED <<seq, expired, hold, heldOnce, unexOnce, ikept, iwritten, isnap, wvars,
                   rpc, rrec, rhad, rdec, later, overtaken, latestLost>>

----------------------------------------------------------------------------
(* The reconciler: read the queue position and the latest record; act -     *)
(* collect, stage a rewrite, or purge what the latest supersedes; then look *)
(* again and queue anything unsettled.                                      *)

RRead(r) ==
    /\ rpc[r] = "idle"
    /\ queued
    /\ queued' = FALSE
    /\ rclaim' = [rclaim EXCEPT ![r] = TRUE]
    /\ rhad' = [rhad EXCEPT ![r] = Present]
    /\ rrec' = [rrec EXCEPT ![r] = LatestOr]
    /\ rpc' = [rpc EXCEPT ![r] = "act"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars,
                   wvars, rdec, backup, later, overtaken, avars, latestLost>>

\* - Collect: the decision is one transactional statement that finds no owner, no pending writer, no staged
\*   rewrite and no hold.
\* - Rewrite: the latest the read saw holds deleted spans; a rewrite of it is staged, unless one is.
\* - Supersede: delete every version below the latest the read saw. On the strength of the read - a version
\*   this purge deletes was below a version that existed - and bounded by it, so it never reaches the latest
\*   the readers see now unless that latest is itself below the read, which only a collection's purge produces
\*   and the ghost records.
\* - No record: the entry's record is not stored; rows naming it that no span owns are a dead writer's
\*   leftovers after a purge, and go. A row of a grant racing this delete is cache a re-derivation rebuilds.
RAct(r) ==
    /\ rpc[r] = "act"
    /\ \/ /\ rhad[r] /\ aown = {} /\ apend = {} /\ ~rwpend /\ ~hold
          /\ acoll' = acoll \cup {aepoch}
          /\ everColl' = everColl \cup {aepoch}
          /\ rdec' = [rdec EXCEPT ![r] = aepoch]
          /\ aepoch' = IF Epochs THEN aepoch + 1 ELSE aepoch
          /\ rpc' = [rpc EXCEPT ![r] = "purge"]
          /\ UNCHANGED <<rows, store, seq, aown, apend, aret, rwpend, rwcontent, rwid, latestLost>>
       \/ /\ rhad[r] /\ (aown # {} \/ apend # {} \/ rwpend \/ hold)
          /\ rrec[r].content \cap tomb # {}
          /\ IF rwpend
               THEN UNCHANGED <<rwpend, rwcontent, rwid>>
               ELSE /\ rwpend' = TRUE
                    /\ rwcontent' = rrec[r].content \ tomb
                    /\ rwid' = rwid + 1
          /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
          /\ UNCHANGED <<rows, store, seq, aown, apend, aret, aepoch, acoll, everColl, rdec, latestLost>>
       \/ /\ rhad[r] /\ (aown # {} \/ apend # {} \/ rwpend \/ hold)
          /\ rrec[r].content \cap tomb = {}
          /\ store' = {v \in store : ~Below(v, rrec[r])}
          /\ latestLost' = (latestLost \/ (aown # {} /\ Present /\ Latest \notin store'))
          /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
          /\ UNCHANGED <<rows, seq, aown, apend, aret, rwpend, rwcontent, rwid, aepoch, acoll, everColl, rdec>>
       \/ /\ ~rhad[r]
          /\ rows' = rows \cap aown
          /\ store' = {v \in store : ~Doomed(v.ep)}
          /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
          /\ UNCHANGED <<seq, avars, rdec, latestLost>>
    /\ rrec' = [rrec EXCEPT ![r] = NoVersion]
    /\ rhad' = [rhad EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars, wvars,
                   rclaim, backup, later, overtaken>>

\* The physical delete of a decided collection: every version numbered at or below its epoch, and every row of
\* the record whose span is not currently owned - the collected record is wanted by nothing, and a dead
\* writer's rows must not outlive it. The owner set is read in the purge's transactional read, so a grant that
\* races the row delete can lose rows; rows are a cache a re-derivation rebuilds from the record the grant
\* keeps alive. The confirm fence keeps every owned version above every decided epoch, so the version delete
\* can never reach one, however late it runs or however many collectors run it - which the ghost records.
\* Without epochs there is nothing to bound it by, and it takes every version.
RPurge(r) ==
    /\ rpc[r] = "purge"
    /\ store' = IF Epochs THEN {v \in store : v.ep > rdec[r]} ELSE {}
    /\ rows' = rows \cap aown
    /\ latestLost' = (latestLost \/ (aown # {} /\ Present /\ Latest \notin store'))
    /\ rpc' = [rpc EXCEPT ![r] = "drop"]
    /\ UNCHANGED <<seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars, wvars,
                   rrec, rhad, rclaim, rdec, backup, later, overtaken, avars>>

\* The purge done, the decision is dropped: readers need not ignore its versions any more. The epoch stays in
\* the high-water mark, so a version a straggling purge of this decision could still delete remains findable.
RDrop(r) ==
    /\ rpc[r] = "drop"
    /\ acoll' = acoll \ {rdec[r]}
    /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   wvars, rrec, rhad, rclaim, rdec, backup, later, overtaken,
                   aown, apend, aret, rwpend, rwcontent, rwid, aepoch, everColl, latestLost>>

\* A collection whose collector died - or is merely slow: the takeover is decided by the decision's age, so it
\* must be safe while its first collector lives - is purged again by another.
RResume(r) ==
    /\ rpc[r] = "idle"
    /\ \E e \in acoll :
        /\ rdec' = [rdec EXCEPT ![r] = e]
        /\ rpc' = [rpc EXCEPT ![r] = "purge"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   wvars, rrec, rhad, rclaim, backup, later, overtaken, avars, latestLost>>

\* Anything a pass must not leave behind: deleted content in the latest, versions below it, a record nothing
\* owns, a version some purge is still entitled to.
Unsettled ==
    \/ Present /\ (Latest.content \cap tomb # {} \/ (\E v \in store : Below(v, Latest)) \/ aown = {})
    \/ \E v \in store : Doomed(v.ep)

\* Look again; queue anything unsettled, so whichever reconciler acts last sees its own effect.
RRecheck(r) ==
    /\ rpc[r] = "recheck"
    /\ IF Unsettled THEN Enqueue ELSE UNCHANGED queued
    /\ rpc' = [rpc EXCEPT ![r] = "clear"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars,
                   rrec, rhad, rclaim, rdec, backup, later, overtaken, avars, latestLost>>

\* Deletes exactly the entry this pass claimed - but only after the landing horizon, with one more look. The
\* horizon is enforced at the landing itself, by the backend: every delayed write carries a start deadline the
\* backend evaluates on its own clock plus the backend's execution-time cap (see Land), so no request can
\* physically commit past deadline + cap, whatever happened to the client that submitted it. The clear waits
\* that horizon out on the same clock and re-checks the record, re-queueing it if a landing arrived meanwhile.
\* Modeled as: no delayed landing is in flight, and the clear performs the recheck.
RClear(r) ==
    /\ rpc[r] = "clear"
    /\ \A i \in Ingests : ipc[i] \notin {"write", "rekey", "repair"}
                           /\ (ipc[i] = "land" => isnap[i].present)
    /\ \A w \in Rewriters : wpc[w] # "append"
    /\ IF Unsettled THEN Enqueue ELSE UNCHANGED queued
    /\ rclaim' = [rclaim EXCEPT ![r] = FALSE]
    /\ rdec' = [rdec EXCEPT ![r] = 0]
    /\ rpc' = [rpc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars,
                   rrec, rhad, backup, later, overtaken, avars, latestLost>>

\* The reconciler's process dies between steps. What it read goes with it; its queue entries stay, uncleared,
\* and a collection it decided stays recorded for another to purge.
RCrash(r) ==
    /\ WithCrash
    /\ rpc[r] \in {"act", "purge", "drop", "recheck", "clear"}
    /\ rpc' = [rpc EXCEPT ![r] = "idle"]
    /\ rrec' = [rrec EXCEPT ![r] = NoVersion]
    /\ rhad' = [rhad EXCEPT ![r] = FALSE]
    /\ rdec' = [rdec EXCEPT ![r] = 0]
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, queued, ivars,
                   wvars, rclaim, backup, later, overtaken, avars, latestLost>>

\* An entry claimed by a reconciler that died is re-queued by age - the claiming protocol's takeover.
RequeueOrphan(r) ==
    /\ rpc[r] = "idle"
    /\ rclaim[r]
    /\ rclaim' = [rclaim EXCEPT ![r] = FALSE]
    /\ Enqueue
    /\ UNCHANGED <<rows, store, seq, tomb, expired, hold, heldOnce, unexOnce, frozen, ivars, wvars,
                   rrec, rhad, rdec, rpc, backup, later, overtaken, avars, latestLost>>

Next ==
    \/ \E i \in Ingests : \/ Fence(i) \/ Register(i) \/ Insert(i) \/ Look(i) \/ Land(i) \/ Write(i) \/ Fail(i)
                          \/ Confirm(i) \/ Rekey(i) \/ Check(i) \/ Repair(i) \/ Settle(i) \/ IDie(i)
    \/ \E w \in Rewriters : RewriteLook(w) \/ RewriteAppend(w) \/ RewriteSettle(w)
    \/ \E s \in Spans : Delete(s) \/ ExpireMark(s) \/ Supersede(s)
    \/ Unexpire
    \/ Sweep \/ ExpireSweep \/ MaintSweep \/ SetHold \/ ReleaseHold \/ Backup \/ Restore
    \/ \E r \in Reconcilers : RRead(r) \/ RAct(r) \/ RPurge(r) \/ RDrop(r) \/ RResume(r) \/ RRecheck(r)
                              \/ RClear(r) \/ RCrash(r) \/ RequeueOrphan(r)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Settled: every ingest and rewrite finished, no reconciler mid-step, the  *)
(* queue empty, no decision left to purge, no row for a deleted span, no    *)
(* sweep owed, and no version a straggling purge is still entitled to.      *)
Drained == ~queued /\ \A r \in Reconcilers : ~rclaim[r]

Settled ==
    /\ \A i \in Ingests : ipc[i] = "done"
    /\ \A w \in Rewriters : wpc[w] = "idle"
    /\ ~rwpend
    /\ \A r \in Reconcilers : rpc[r] = "idle"
    /\ Drained
    /\ acoll = {}
    /\ rows \cap (tomb \cup expired) = {}
    /\ aown \cap expired = {}
    /\ \A v \in store : ~Doomed(v.ep)

LiveRows == rows \ (tomb \cup expired)

Covered == Settled => (LiveRows # {} => Present /\ LiveRows \subseteq Latest.content)

\* Deleted content is in no stored version - not only the latest - once settled.
Private == Settled => \A v \in store : v.content \cap tomb = {}

Collected == Settled => (aown = {} => store = {})

\* Under a hold, nothing live and not deleted when it began is lost. (A restore under a hold rebases `frozen`
\* in its own step: the operator's restore is the one permitted change, documented at the action.)
HeldIntact == hold => (frozen.content \ frozen.tomb) \subseteq LatestContent

\* Every version is the export minus some deleted spans.
VersionsAreFiltered == \A v \in store : Spans \ v.content \subseteq tomb

\* A span a later revision has taken over is never this export's again.
LaterReceiptWins == ~overtaken

\* Content a live row needs and the latest record lacks is owed only by something durable or still running: an
\* execution yet to check or repair, one refused and to be delivered again, or a staged rewrite that carries
\* the content. Checked whenever the record is not mid-collection - a queued entry or a pending decision owns
\* the rows' fate (a dead writer's orphan rows go with the purge), and both leave a trace until the pass that
\* resolves them clears it.
Recoverable ==
    (Drained /\ acoll = {} /\ LiveRows # {} /\ ~(LiveRows \subseteq LatestContent)) =>
        \/ \E i \in Ingests : \/ ipc[i] \in {"register", "insert", "look", "land", "write", "confirm",
                                             "rekey", "check", "repair"}
                              \/ ipc[i] = "fence" /\ ikept[i] # {}
        \/ (rwpend /\ LiveRows \subseteq rwcontent)

\* Once the queue is drained, every live row is owned - durably, or by an execution still on its way to
\* confirming it. An unowned live row can exist only while its record is queued: every append and every
\* ownership removal queues the record, so what a dead writer left is processed - and the collection that
\* takes an unowned record takes its unowned rows with it.
OwnersCoverRows ==
    Drained /\ acoll = {} =>
        LiveRows \subseteq aown \cup UNION {ikept[i] : i \in {j \in Ingests :
            Token(j) \in apend \/ ipc[j] \in {"confirm", "rekey"}}}

\* No purge - a collection's, however late or taken over, or a supersede on a stale read - deletes the latest
\* version of a record its rows own. The confirm fence is what makes this an invariant rather than a race.
NoLatestLost == ~latestLost

\* What a reader that reaches a user sees: the latest content, gated by live ownership in the transactional
\* store, behind the retention predicate every reader applies. A deleted span - whose record a stale write may
\* have brought back - is never owned again and never in the view; an expired span may be owned until the sweep
\* takes it, and the predicate hides it meanwhile.
GatedView == (LatestContent \cap aown) \ expired
NoResurrectedRead == GatedView \cap tomb = {} /\ aown \cap tomb = {}

\* A collected record's bytes back in storage because a stale write landed after the purge. Not an invariant -
\* the landing is reachable - but it is never a resurrection a reader can see: readers take only versions above
\* the collected high-water mark, so the bytes are invisible by construction, excluded from `Settled`, findable
\* by the maintenance scan through their own version key, and purged by a later pass. Add NeverResurrected to a
\* configuration's INVARIANTS to see TLC print such a landing.
Resurrected == \E v \in store : Doomed(v.ep)
NeverResurrected == ~Resurrected

\* The model is symmetric in spans, the rewrite executions and reconcilers - not in ingests when they share a
\* payload, which `Token` picks one of.
Symmetry == Permutations(Spans) \cup Permutations(Rewriters) \cup Permutations(Reconcilers)

\* Bounds for the model checker: versions and epochs are finite. The queue needs none - entries dedup to one
\* unclaimed boolean plus a claim per reconciler.
StateBound == seq <= 4 /\ aepoch <= 2
=============================================================================
