------------------------ MODULE RawRecordOwnership ------------------------
(***************************************************************************)
(* The life of one raw record: the stored export a set of span rows is     *)
(* derived from, under redelivery, deletion, retention, legal hold,         *)
(* backup and restore, with a reconciler that removes deleted content.     *)
(*                                                                         *)
(* One export carries the spans `Spans`. Every write of its record - the   *)
(* first insert, an ingest's repair, a reconciler's rewrite - stores the   *)
(* export filtered to a subset of `Spans`; a store keeps every version and *)
(* readers take the latest by (version, insertion order), which is what    *)
(* DuckDB's append-only table and ClickHouse's ReplacingMergeTree(version) *)
(* both answer. Nothing is atomic across the record, the rows and the      *)
(* tombstones: each step below is one statement in the implementation.     *)
(*                                                                         *)
(* What the protocol has to hold, and the mechanism for each:              *)
(*                                                                         *)
(*   Covered    - every span row's content is in the latest record. An     *)
(*                ingest that finds the record absent or missing its spans *)
(*                after writing its rows appends the union of the latest   *)
(*                record and its own kept spans, at least two versions     *)
(*                above what it read, so a reconciler's rewrite (one above *)
(*                what *it* read) of an older version can never win over   *)
(*                it, whatever the writers' clocks say.                    *)
(*   Private    - once settled, the record holds no deleted span. Every    *)
(*                deletion of rows enqueues the record; the reconciler     *)
(*                rewrites it without the tombstoned spans.                *)
(*   Collected  - once settled, a record no row names is gone. Retention   *)
(*                enqueues like deletion, and so does a failed write; a    *)
(*                reconciler that deleted the record re-checks the rows    *)
(*                and restores what it read if any appeared.               *)
(*   HeldIntact - under a legal hold no record content is lost: deletion   *)
(*                and retention are refused, the reconciler waits, and an  *)
(*                ingest's repair only adds.                              *)
(*   LaterReceiptWins - an ingest of this export that writes its rows after *)
(*                a revision received later took a span over never takes   *)
(*                it back: with ReceiptPrecedence a row's instant is its    *)
(*                export's receipt, so the late row is superseded at once. *)
(*                Without it the row's instant is the write's, and an      *)
(*                ingest's late replay wins over the later revision.       *)
(*   Recoverable - a live row's content missing from the latest record is  *)
(*                only ever owed by a process that holds it and has yet to *)
(*                act: the ingest still to check, or a reconciler that read *)
(*                the record. A reconciler can die between its steps       *)
(*                (WithCrash), so the one that deleted on a stale read may *)
(*                never look again: with two calls - read who names the    *)
(*                record, then delete it - an ingest whose rows and check   *)
(*                fall between them loses its record for good. AtomicDelete *)
(*                deletes in one step that finds no row, as DuckDB's single *)
(*                connection allows, and the hole is gone.                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Spans, Ingests, Reconcilers, MaxVer,
          WithHold,          \* explore legal holds
          WithRestore,       \* explore backup and restore
          ReceiptPrecedence, \* a row's instant is its export's receipt, not its write
          AtomicDelete,      \* the reconciler deletes in one step that finds no row naming the record
          WithCrash          \* a reconciler can die between its steps

ASSUME Spans # {} /\ Ingests # {} /\ Reconcilers # {} /\ MaxVer \in Nat
ASSUME WithHold \in BOOLEAN /\ WithRestore \in BOOLEAN /\ ReceiptPrecedence \in BOOLEAN
ASSUME AtomicDelete \in BOOLEAN /\ WithCrash \in BOOLEAN

VARIABLES
    rows,      \* spans that currently have a row naming the record
    store,     \* every stored version: [ver, seq, content]
    seq,       \* insertion order, the tie-break between equal versions
    tomb,      \* spans removed by a deletion: tombstoned or journalled, never shrinks
    hold,      \* a project legal hold is active
    heldOnce,  \* one hold per behaviour: a second adds states, not interleavings
    frozen,    \* the latest content when the hold began (history variable)
    enq,       \* reconciliation entries enqueued so far
    cleared,   \* entries up to this one are processed
    ipc, ikept, iwritten,
    rpc, rrec, rhad, rlive, rtaken,
    rtomb,     \* the deletion fences a reconciler read for its rewrite, after its delete
    backup,    \* one snapshot of the analytics store, when `taken`
    later,     \* spans a revision received after this export has been written for
    overtaken  \* ghost: a row of this export was written over a revision received after it

vars == <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept, iwritten,
          rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

Version == [ver : 0..MaxVer, seq : Nat, content : SUBSET Spans]

Present == store # {}

LatestOf(S) == CHOOSE r \in S :
                 \A o \in S : o.ver < r.ver \/ (o.ver = r.ver /\ o.seq <= r.seq)

Latest == LatestOf(store)

LatestContent == IF Present THEN Latest.content ELSE {}
LatestVer == IF Present THEN Latest.ver ELSE 0

Append(ver, content) ==
    /\ store' = store \cup {[ver |-> ver, seq |-> seq, content |-> content]}
    /\ seq' = seq + 1

Enqueue == enq' = enq + 1

TypeOK ==
    /\ rows \subseteq Spans
    /\ store \subseteq Version
    /\ tomb \subseteq Spans
    /\ hold \in BOOLEAN
    /\ cleared <= enq
    /\ ipc \in [Ingests -> {"fence", "insert", "write", "check", "done"}]
    /\ rpc \in [Reconcilers -> {"idle", "act", "rewrite", "recheck", "clear"}]

Init ==
    /\ rows = {}
    /\ store = {}
    /\ seq = 0
    /\ tomb = {}
    /\ hold = FALSE
    /\ heldOnce = FALSE
    /\ frozen = {}
    /\ enq = 0
    /\ cleared = 0
    /\ ipc = [i \in Ingests |-> "fence"]
    /\ ikept = [i \in Ingests |-> {}]
    /\ iwritten = [i \in Ingests |-> {}]
    /\ rpc = [r \in Reconcilers |-> "idle"]
    /\ rrec = [r \in Reconcilers |-> [ver |-> 0, seq |-> 0, content |-> {}]]
    /\ rhad = [r \in Reconcilers |-> FALSE]
    /\ rlive = [r \in Reconcilers |-> {}]
    /\ rtaken = [r \in Reconcilers |-> 0]
    /\ rtomb = [r \in Reconcilers |-> {}]
    /\ backup = [taken |-> FALSE, rows |-> {}, store |-> {}, pending |-> {}]
    /\ later = {}
    /\ overtaken = FALSE

----------------------------------------------------------------------------
(* Ingest: the deletion fences decide what is kept, the record is inserted  *)
(* if absent, the rows are written, and the record is checked against them. *)

Fence(i) ==
    /\ ipc[i] = "fence"
    /\ ikept' = [ikept EXCEPT ![i] = Spans \ tomb]
    /\ ipc' = [ipc EXCEPT ![i] = IF Spans \ tomb = {} THEN "done" ELSE "insert"]
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

\* Insert-if-absent, at whatever version the writer's clock gives: a clock far behind the others or far
\* ahead of them, the two extremes skew can produce.
Insert(i) ==
    /\ ipc[i] = "insert"
    /\ IF Present
         THEN UNCHANGED <<store, seq>>
         ELSE \E v \in {1, MaxVer} : Append(v, ikept[i])
    /\ ipc' = [ipc EXCEPT ![i] = "write"]
    /\ UNCHANGED <<rows, tomb, hold, heldOnce, frozen, enq, cleared, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

\* No fence between the record and the rows: a deletion in between leaves
\* rows the existing tombstone re-check removes (Sweep).
\* With ReceiptPrecedence a row's instant is its export's receipt, so a row written where a revision received later
\* already is goes in superseded; without it the row's instant is the write's, and it takes the span back.
Overtakes(spans) == ~ReceiptPrecedence /\ spans \cap later # {}

Write(i) ==
    /\ ipc[i] = "write"
    /\ rows' = rows \cup ikept[i]
    /\ overtaken' = (overtaken \/ Overtakes(ikept[i]))
    /\ UNCHANGED later
    /\ iwritten' = [iwritten EXCEPT ![i] = ikept[i]]
    /\ ipc' = [ipc EXCEPT ![i] = "check"]
    /\ UNCHANGED <<store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ikept,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup>>

\* The span write fails. A backend can have stored its rows all the same - a ClickHouse insert whose view
\* throws after the block is written, one whose answer was lost - or none of them, and the caller cannot tell
\* which. The record it stored before them may now be named by nothing, so it is queued for the reconciler;
\* the export is refused and delivered again, from the fences. No repair runs: the write did not return.
Fail(i) ==
    /\ ipc[i] = "write"
    /\ \E landed \in {{}, ikept[i]} :
        /\ rows' = rows \cup landed
        /\ overtaken' = (overtaken \/ Overtakes(landed))
    /\ UNCHANGED later
    /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "fence"]
    /\ UNCHANGED <<store, seq, tomb, hold, heldOnce, frozen, cleared, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup>>

\* The repair: absent or not covering, append the union two versions up. Exactly two: a repair further up
\* only wins more often, so the least is the case that has to hold.
\*
\* The repair is appended and queued in one step, for the reason RRecheck gives.
\*
\* The order is load-bearing: the rows, then this check, then the acknowledgement (the ingest is done). With
\* AtomicDelete a record deleted before the rows were written is absent here and written again from the ingest's
\* own bytes, and one the rows were written before is not deleted at all; a check before the rows, or an
\* acknowledgement before the check, would leave a row whose record a delete took in between.
Check(i) ==
    /\ ipc[i] = "check"
    /\ IF Present /\ iwritten[i] \subseteq Latest.content
         THEN UNCHANGED <<store, seq, enq>>
         ELSE /\ LatestVer + 2 <= MaxVer
              /\ Append(LatestVer + 2, LatestContent \cup ikept[i])
              /\ Enqueue
    /\ ipc' = [ipc EXCEPT ![i] = "done"]
    /\ UNCHANGED <<rows, tomb, hold, heldOnce, frozen, cleared, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

----------------------------------------------------------------------------
(* Deletion, the tombstone sweep, retention and holds.                     *)

\* Tombstone and journal, enqueue the record, then delete the rows. The record is found through the
\* trace index (`otel_raw_traces`), not through the rows: a span whose row already expired, or a
\* superseded revision ClickHouse has merged away, names nothing, and the record must still be found.
Delete(s) ==
    /\ ~hold
    /\ s \notin tomb
    /\ tomb' = tomb \cup {s}
    /\ rows' = rows \ {s}
    \* Every revision of the identity goes.
    /\ later' = later \ {s}
    /\ Enqueue
    /\ UNCHANGED <<store, seq, hold, heldOnce, frozen, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, overtaken>>

\* The existing compensating re-check: rows written for a deleted span go again.
Sweep ==
    /\ rows \cap tomb # {}
    /\ rows' = rows \ tomb
    /\ UNCHANGED <<later, overtaken>>
    /\ Enqueue
    /\ UNCHANGED <<store, seq, tomb, hold, heldOnce, frozen, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup>>

Expire(s) ==
    /\ ~hold
    /\ s \in rows
    /\ rows' = rows \ {s}
    /\ later' = later \ {s}
    /\ Enqueue
    /\ UNCHANGED <<store, seq, tomb, hold, heldOnce, frozen, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, overtaken>>

SetHold ==
    /\ WithHold
    /\ ~hold
    /\ ~heldOnce
    /\ hold' = TRUE
    /\ heldOnce' = TRUE
    /\ frozen' = LatestContent
    /\ UNCHANGED <<rows, store, seq, tomb, enq, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

ReleaseHold ==
    /\ hold
    /\ hold' = FALSE
    /\ frozen' = {}
    /\ UNCHANGED <<rows, store, seq, tomb, heldOnce, enq, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

----------------------------------------------------------------------------
(* Backup and restore. The journal and tombstones live in the transactional *)
(* store and survive; the analytics store (rows, records, queue) goes back  *)
(* to the snapshot. Restore enqueues every restored record, and its replay  *)
(* of the journal is the Sweep.                                            *)

\* With AtomicDelete the store is DuckDB's, which is backed up and restored with the server stopped
\* (`backup-restore.mdx`): no ingest or reconciler is part-way through, and SQLite is backed up with it, so an
\* export refused and still staged for redelivery at the backup is staged again after the restore.
Quiescent ==
    /\ \A i \in Ingests : ipc[i] \in {"fence", "done"}
    /\ \A r \in Reconcilers : rpc[r] = "idle"

Backup ==
    /\ WithRestore
    /\ AtomicDelete => Quiescent
    /\ ~backup.taken
    /\ backup' = [taken |-> TRUE, rows |-> rows, store |-> store,
                  pending |-> IF AtomicDelete THEN {i \in Ingests : ipc[i] = "fence" /\ ikept[i] # {}} ELSE {}]
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept,
                   iwritten, rpc, rrec, rhad, rlive, rtaken, rtomb, later, overtaken>>

\* A restore can take a later revision's rows back with it; `later` keeps them all the same. That over-approximates
\* what a write can overtake, which matters only without ReceiptPrecedence, and keeps the snapshot to the rows and
\* the store.
Restore ==
    /\ backup.taken
    /\ AtomicDelete => Quiescent
    /\ rows' = backup.rows
    /\ store' = backup.store
    /\ ipc' = [i \in Ingests |-> IF i \in backup.pending THEN "fence" ELSE ipc[i]]
    /\ Enqueue
    /\ backup' = [taken |-> FALSE, rows |-> {}, store |-> {}, pending |-> {}]
    \* An operator's restore under a hold is what the hold now protects.
    /\ frozen' = IF hold /\ backup.store # {} THEN LatestOf(backup.store).content ELSE {}
    /\ UNCHANGED <<seq, tomb, hold, heldOnce, cleared, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, later, overtaken>>

\* Another export, received after this one, writes its revision of a span: the span's winner is no longer ours.
Supersede(s) ==
    /\ s \notin tomb
    /\ s \notin later
    /\ later' = later \cup {s}
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept, iwritten,
                   rpc, rrec, rhad, rlive, rtaken, rtomb, backup, overtaken>>

----------------------------------------------------------------------------
(* The reconciler: read the queue position, the latest record and the rows; *)
(* act unless a hold is active; after a delete, re-check the rows.         *)

RRead(r) ==
    /\ rpc[r] = "idle"
    /\ enq > cleared
    /\ rtaken' = [rtaken EXCEPT ![r] = enq]
    /\ rhad' = [rhad EXCEPT ![r] = Present]
    /\ rrec' = [rrec EXCEPT ![r] = IF Present THEN Latest
                                   ELSE [ver |-> 0, seq |-> 0, content |-> {}]]
    /\ rlive' = [rlive EXCEPT ![r] = rows]
    /\ rpc' = [rpc EXCEPT ![r] = "act"]
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept,
                   iwritten, rtomb, backup, later, overtaken>>

\* With AtomicDelete the delete is one step that finds no row naming the record - rows written since the read keep
\* it - and a record it finds named is rewritten in a step of its own (RRewrite), stored only while a version of
\* the record is. Without it, both are decided from the read, and a reconciler that dies after acting on it leaves
\* what it did unchecked: a record deleted under rows that appeared since, or one rewritten back after another
\* reconciler collected it.
RAct(r) ==
    /\ rpc[r] = "act"
    /\ ~hold
    /\ IF AtomicDelete
         THEN \/ /\ ~rhad[r]
                 /\ UNCHANGED <<store, seq, rtomb>>
                 /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
              \/ /\ rhad[r] /\ rows = {}
                 /\ store' = {}
                 /\ UNCHANGED <<seq, rtomb>>
                 /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
              \* Named: the rewrite reads the deletion fences now and appends later, in steps of its own.
              \/ /\ rhad[r] /\ rows # {}
                 /\ UNCHANGED <<store, seq>>
                 /\ rtomb' = [rtomb EXCEPT ![r] = tomb]
                 /\ rpc' = [rpc EXCEPT ![r] = "rewrite"]
         ELSE /\ UNCHANGED rtomb
              /\ \/ /\ ~rhad[r]
                    /\ UNCHANGED <<store, seq>>
                 \/ /\ rhad[r] /\ rlive[r] = {}
                    /\ store' = {}
                    /\ UNCHANGED seq
                 \/ /\ rhad[r] /\ rlive[r] # {}
                    /\ LET new == rrec[r].content \ tomb IN
                         IF new = rrec[r].content
                           THEN UNCHANGED <<store, seq>>
                           ELSE /\ rrec[r].ver + 1 <= MaxVer
                                /\ Append(rrec[r].ver + 1, new)
              /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
    /\ UNCHANGED <<rows, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept, iwritten,
                   rrec, rhad, rlive, rtaken, backup, later, overtaken>>

\* The record read, without what the deletion fences refused when they were read, one version up - appended only
\* while a version of the record is stored, and queued, in the same step. A deletion since the fences were read
\* stays in it and can win the version tie over a newer rewrite after that one's reconciler cleared the entries; the
\* entry this rewrite queues brings a reconciler back to it, whether or not this one lives to look again.
RRewrite(r) ==
    /\ rpc[r] = "rewrite"
    /\ ~hold
    /\ LET new == rrec[r].content \ rtomb[r] IN
         IF new = rrec[r].content \/ ~Present
           THEN UNCHANGED <<store, seq, enq>>
           ELSE /\ rrec[r].ver + 1 <= MaxVer
                /\ Append(rrec[r].ver + 1, new)
                /\ Enqueue
    /\ rpc' = [rpc EXCEPT ![r] = "recheck"]
    /\ UNCHANGED <<rows, tomb, hold, heldOnce, frozen, cleared, ipc, ikept, iwritten,
                   rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

\* After every action, look again. Rows that appeared after this reconciler's delete
\* get back what it read - every version is the export minus some deleted spans, so it covers
\* them - appended and queued in one step: queued first, another reconciler can clear the entry
\* before the append, and the restored record is left with nothing to look at it again; queued
\* after, a reconciler that dies between the two leaves the same. Anything else unsettled - a
\* record no row names, deleted content, which a concurrent reconciler acting on an older read
\* can leave - is enqueued again, so whichever reconciler acts last sees its own effect.
RRecheck(r) ==
    /\ rpc[r] = "recheck"
    /\ IF rows # {} /\ ~Present /\ rhad[r]
         THEN /\ rrec[r].ver + 1 <= MaxVer
              /\ Append(rrec[r].ver + 1, rrec[r].content)
              /\ Enqueue
         ELSE /\ UNCHANGED <<store, seq>>
              /\ IF (rows = {} /\ Present) \/ (rows # {} /\ ~Present)
                    \/ (LatestContent \cap tomb # {})
                   THEN Enqueue
                   ELSE UNCHANGED enq
    /\ rpc' = [rpc EXCEPT ![r] = "clear"]
    /\ UNCHANGED <<rows, tomb, hold, heldOnce, frozen, cleared, ipc, ikept, iwritten,
                   rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

RClear(r) ==
    /\ rpc[r] = "clear"
    /\ cleared' = IF rtaken[r] > cleared THEN rtaken[r] ELSE cleared
    /\ rpc' = [rpc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, ipc, ikept, iwritten,
                   rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

\* The reconciler's process dies between steps. What it read goes with it; its queue entries stay, uncleared,
\* for the next reconciler.
RCrash(r) ==
    /\ WithCrash
    /\ rpc[r] \in {"act", "rewrite", "recheck", "clear"}
    /\ rpc' = [rpc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<rows, store, seq, tomb, hold, heldOnce, frozen, enq, cleared, ipc, ikept, iwritten,
                   rrec, rhad, rlive, rtaken, rtomb, backup, later, overtaken>>

Next ==
    \/ \E i \in Ingests : Fence(i) \/ Insert(i) \/ Write(i) \/ Fail(i) \/ Check(i)
    \/ \E s \in Spans : Delete(s) \/ Expire(s) \/ Supersede(s)
    \/ Sweep \/ SetHold \/ ReleaseHold \/ Backup \/ Restore
    \/ \E r \in Reconcilers : RRead(r) \/ RAct(r) \/ RRewrite(r) \/ RRecheck(r) \/ RClear(r) \/ RCrash(r)

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Settled: every ingest finished, no reconciler mid-step, the queue empty, *)
(* no row for a deleted span.                                              *)
Settled ==
    /\ \A i \in Ingests : ipc[i] = "done"
    /\ \A r \in Reconcilers : rpc[r] = "idle"
    /\ enq = cleared
    /\ rows \cap tomb = {}

Covered == Settled => (rows # {} => Present /\ rows \subseteq Latest.content)

Private == Settled => LatestContent \cap tomb = {}

Collected == Settled => (rows = {} => ~Present)

\* Under a hold nothing that was there when it began is lost.
HeldIntact == hold => frozen \subseteq LatestContent

\* The repair and the rewrite keep the shape every version has: the export
\* minus spans that were deleted. That is why a re-inserted read covers rows.
VersionsAreFiltered == \A v \in store : Spans \ v.content \subseteq tomb

\* A span a later revision has taken over is never this export's again.
LaterReceiptWins == ~overtaken

\* Content a live row needs and the latest record lacks is owed only by a process that holds it and has yet to act:
\* an ingest still to check, or refused and to be delivered again, or a reconciler that read the record. Checked in
\* every state, not only once settled: a reconciler that dies after a stale delete leaves the queue entry behind,
\* and the next one finds nothing to restore, so the record is lost although no state ever settles.
Recoverable ==
    LET live == rows \ tomb IN
    (live # {} /\ ~(live \subseteq LatestContent)) =>
        \/ \E i \in Ingests : \/ ipc[i] \in {"insert", "write", "check"}
                              \/ ipc[i] = "fence" /\ ikept[i] # {}
        \/ \E r \in Reconcilers : rpc[r] \in {"act", "rewrite", "recheck"} /\ rhad[r]

\* The model is symmetric in spans, ingests and reconcilers.
Symmetry == Permutations(Spans) \cup Permutations(Ingests) \cup Permutations(Reconcilers)

\* Bounds for the model checker: versions and queue entries are finite.
StateBound == seq <= 4 /\ enq <= 3
=============================================================================
