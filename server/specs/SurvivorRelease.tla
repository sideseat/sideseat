----------------------------- MODULE SurvivorRelease -----------------------------
(***************************************************************************)
(* Releasing a trace's file association its surviving spans no longer      *)
(* reference, against writers that commit spans naming the file and a      *)
(* retention that deletes them.                                            *)
(*                                                                         *)
(* One trace, one file. A committed span names the file, its association   *)
(* durable. A writer - an ingest batch - associates the file (one pending  *)
(* writer more), writes its span to the analytics store, and confirms      *)
(* (durable, one writer less); or aborts before writing, which resolves    *)
(* its writer and removes a provisional association. Retention deletes a   *)
(* committed span after recording the trace's cleanup under a fresh token. *)
(* Sweeps claim the recorded cleanup by its token - a lease can lapse, so  *)
(* two can claim one - reconcile, and complete it only while the row still *)
(* carries the token they claimed; a sweep that left an association it     *)
(* could not justify (deferred) completes nothing. The raw record names    *)
(* the file too, until a rewrite removes the span that carried it and      *)
(* queues the record; a reconciler pass reads the queue, reconciles, keeps *)
(* a deferred trace's cleanup owed under a fresh token, and clears the     *)
(* entries it read. With `Defer` FALSE, a reconciler that skipped an       *)
(* association for a pending writer treats the cleanup as done, and the   *)
(* writer's abort leaves the file associated and owed nothing.             *)
(*                                                                         *)
(* The survivor scan reads the analytics store, which no transaction of    *)
(* the transactional store covers: a span committed after the scan is not  *)
(* in it. Three protocols for that window; only the last keeps the media.  *)
(*                                                                         *)
(* `ReleaseFirst`, the protocol `CheckFirst` replaced: release what the   *)
(* scan did not keep and no writer is pending on, re-scan, and restore     *)
(* what a span now names. The                                              *)
(* re-check can fail, and then the restore - with the cleanup kept owed,   *)
(* one transaction - is the compensation, which can fail as well. A       *)
(* restore after a successful re-check lands alone.                        *)
(*                                                                         *)
(* `IntentFirst`: the pass records the cleanup under a fresh token before  *)
(* its scan, restores in the transaction that records a fresh one again,   *)
(* and repairs a referenced file the trace lost its association with. The  *)
(* cleanup is never lost, but a referenced association is still released  *)
(* for a while, and an orphan sweep in that while reclaims the bytes.      *)
(*                                                                         *)
(* `CheckFirst`: a confirm stamps the association with a sequence value,   *)
(* and the pass reads the sequence before its scan. The release, one       *)
(* transaction, takes only an association no writer is pending on, the     *)
(* scan did not keep, and no confirm has stamped since that read: a writer *)
(* whose stamp is older wrote its span before the read, so the scan saw    *)
(* it. Nothing is released that a re-check would restore, so there is no   *)
(* re-check and no compensation. `Stamp` FALSE drops the stamp condition.  *)
(*                                                                         *)
(* A confirm draws its stamp, then applies it - PostgreSQL's `nextval` is  *)
(* outside the transaction and one statement updates row after row, so a  *)
(* pass can read the sequence and a second writer confirm in between. The  *)
(* stamp it applies is the larger of the row's and its own (`Max`): with   *)
(* `Max` FALSE an older confirm overwrites a newer one's stamp, and the    *)
(* release takes the newer writer's association.                           *)
(*                                                                         *)
(* Assumed: a span whose write returned before a scan starts is visible   *)
(* to it. DuckDB reads its own committed rows. ClickHouse's survivor scans *)
(* read every replica of every shard (`clusterAllReplicas` over the        *)
(* `_local` tables, `every_replica_spans`), failing rather than skipping   *)
(* an unreachable one, and a span write returns only once a replica holds  *)
(* it (`insert_distributed_sync = 1`, async inserts pinned to wait) - so   *)
(* the scan sees it whatever the replication lag or `insert_quorum`.       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Writers, Sweeps, Passes,
          MaxToken,   \* bound on cleanup tokens, queue entries and sequence values, for the model checker
          Protocol,   \* "ReleaseFirst", "IntentFirst" or "CheckFirst"
          Stamp,      \* with "CheckFirst": the release skips an association confirmed since the pass's read
          Defer,      \* a reconciler that left an association unjustified keeps the trace's cleanup owed
          Max         \* a confirm applies the larger of the association's stamp and its own

ASSUME Protocol \in {"ReleaseFirst", "IntentFirst", "CheckFirst"} /\ Stamp \in BOOLEAN /\ Defer \in BOOLEAN
ASSUME Max \in BOOLEAN
ASSUME MaxToken \in Nat /\ Sweeps \cap Passes = {}

\* The retention sweeps, which claim a recorded cleanup, and the reconciler passes, which read the raw-record queue.
Reconcilers == Sweeps \cup Passes

\* The span committed before the model starts.
Spans == Writers \cup {"s0"}

VARIABLES
    live,       \* the committed spans naming the file
    named,      \* the raw record's latest version names the file
    queue,      \* the raw record's queued entries
    collected,  \* the file's bytes have been reclaimed: what an orphan sweep does to a file no trace is associated with
    assoc, durable, pending,
    stamped,    \* the sequence value of the association's latest confirm
    seq,        \* the confirm sequence's next value
    owed, next, \* the token the recorded cleanup carries (0 for none), and the next token
    wpc,        \* each writer's step
    wstamp,     \* each writer's drawn stamp, 0 before its draw
    spc, stok, sepoch, skeep, sreleased, sdeferred  \* each reconciler's step, token or entries, read, scan, release,
                                                    \* deferral

vars == <<live, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, spc, stok, sepoch,
          skeep, sreleased, sdeferred>>
sweepVars == <<spc, stok, sepoch, skeep, sreleased, sdeferred>>

Referenced == live # {} \/ named

TypeOK ==
    /\ live \subseteq Spans /\ assoc \in BOOLEAN /\ durable \in BOOLEAN
    /\ pending \in 0..Cardinality(Writers) /\ stamped \in Nat /\ seq \in Nat
    /\ owed \in 0..MaxToken /\ next \in 1..(MaxToken + 1)
    /\ wpc \in [Writers -> {"idle", "associated", "written", "drawn", "done"}] /\ wstamp \in [Writers -> Nat]

Init ==
    /\ live = {"s0"} /\ named = TRUE /\ queue = {} /\ collected = FALSE /\ assoc = TRUE /\ durable = TRUE /\ pending = 0
    /\ stamped = 0 /\ seq = 1
    /\ owed = 0 /\ next = 1
    /\ wpc = [w \in Writers |-> "idle"] /\ wstamp = [w \in Writers |-> 0]
    /\ spc = [s \in Reconcilers |-> "idle"] /\ stok = [s \in Reconcilers |-> {}]
    /\ sepoch = [s \in Reconcilers |-> 0]
    /\ skeep = [s \in Reconcilers |-> FALSE] /\ sreleased = [s \in Reconcilers |-> FALSE]
    /\ sdeferred = [s \in Reconcilers |-> FALSE]

Associate(w) ==
    /\ wpc[w] = "idle"
    /\ assoc' = TRUE /\ pending' = pending + 1 /\ durable' = (assoc /\ durable)
    /\ collected' = FALSE
    /\ wpc' = [wpc EXCEPT ![w] = "associated"]
    /\ UNCHANGED <<wstamp, live, named, queue, stamped, seq, owed, next, sweepVars>>

WriteSpan(w) ==
    /\ wpc[w] = "associated"
    /\ live' = live \cup {w}
    /\ wpc' = [wpc EXCEPT ![w] = "written"]
    /\ UNCHANGED <<wstamp, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, sweepVars>>

\* The confirm's stamp, drawn after the span is written and before the association is touched.
Draw(w) ==
    /\ wpc[w] = "written" /\ seq <= MaxToken
    /\ wstamp' = [wstamp EXCEPT ![w] = seq] /\ seq' = seq + 1
    /\ wpc' = [wpc EXCEPT ![w] = "drawn"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, owed, next, sweepVars>>

\* The confirm's update of the association, committing: durable, one writer less, stamped.
Confirm(w) ==
    /\ wpc[w] = "drawn"
    /\ pending' = pending - 1 /\ durable' = TRUE
    /\ stamped' = IF Max /\ stamped > wstamp[w] THEN stamped ELSE wstamp[w]
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, seq, owed, next, wstamp, sweepVars>>

Abort(w) ==
    /\ wpc[w] = "associated"
    /\ pending' = pending - 1
    /\ assoc' = (assoc /\ (durable \/ pending - 1 > 0))
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<wstamp, live, named, queue, collected, durable, stamped, seq, owed, next, sweepVars>>

\* Retention: the cleanup recorded under a fresh token, then the span deleted.
Delete(sp) ==
    /\ sp \in live /\ next <= MaxToken
    /\ owed' = next /\ next' = next + 1
    /\ live' = live \ {sp}
    /\ UNCHANGED <<named, queue, collected, assoc, durable, pending, stamped, seq, wpc, wstamp, sweepVars>>

\* A raw rewrite removes the span that carried the file from the record, and queues the record as it lands.
Rewrite ==
    /\ named /\ next <= MaxToken
    /\ named' = FALSE
    /\ queue' = queue \cup {next} /\ next' = next + 1
    /\ UNCHANGED <<live, collected, assoc, durable, pending, stamped, seq, owed, wpc, wstamp, sweepVars>>

\* A sweep claims the recorded cleanup, by the token it carries; a pass reads the queued entries.
Claim(s) ==
    /\ spc[s] = "idle"
    /\ IF s \in Sweeps
          THEN owed # 0 /\ stok' = [stok EXCEPT ![s] = {owed}]
          ELSE queue # {} /\ stok' = [stok EXCEPT ![s] = queue]
    /\ sepoch' = [sepoch EXCEPT ![s] = seq]
    /\ spc' = [spc EXCEPT ![s] = IF Protocol = "IntentFirst" /\ s \in Sweeps THEN "intending" ELSE "claimed"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, skeep, sreleased, sdeferred>>

Scan(s) ==
    /\ spc[s] = "claimed"
    /\ skeep' = [skeep EXCEPT ![s] = Referenced]
    /\ spc' = [spc EXCEPT ![s] = "scanned"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, stok, sepoch, sreleased, sdeferred>>

\* `IntentFirst` only: before its scan, the pass records the cleanup under a fresh token, which it then holds - so
\* a pass that fails at any later step leaves it owed, and a deletion after the scan records a newer one its
\* completion cannot discard.
Intend(s) ==
    /\ spc[s] = "intending" /\ next <= MaxToken
    /\ owed' = next /\ next' = next + 1
    /\ stok' = [stok EXCEPT ![s] = {next}]
    /\ spc' = [spc EXCEPT ![s] = "claimed"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, seq, wpc, wstamp, sepoch, skeep, sreleased, sdeferred>>

Release(s) ==
    /\ spc[s] = "scanned"
    /\ LET fresh == Protocol = "CheckFirst" /\ Stamp /\ stamped >= sepoch[s]
           release == assoc /\ ~skeep[s] /\ pending = 0 /\ ~fresh
           \* With `IntentFirst`, a file the scan saw referenced and the trace is no longer associated with - a
           \* failed compensation's - is repaired in the release's transaction.
           repair == Protocol = "IntentFirst" /\ skeep[s] /\ ~assoc
       IN /\ assoc' = ((assoc /\ ~release) \/ repair) /\ durable' = ((durable /\ ~release) \/ repair)
          /\ sreleased' = [sreleased EXCEPT ![s] = release]
          \* The scan it repairs on may be stale, so the repair keeps the cleanup owed under a fresh token too.
          /\ IF repair
                THEN next <= MaxToken /\ owed' = next /\ next' = next + 1
                ELSE UNCHANGED <<owed, next>>
    /\ spc' = [spc EXCEPT ![s] = "released"]
    /\ UNCHANGED <<live, named, queue, collected, pending, stamped, seq, wpc, wstamp, stok, sepoch, skeep, sdeferred>>

\* What the release left that the scan does not justify: deferred. A failed read counts as deferred.
Left(s) ==
    /\ spc[s] = "released"
    /\ \/ sdeferred' = [sdeferred EXCEPT ![s] = Defer /\ assoc /\ ~skeep[s]]
       \/ sdeferred' = [sdeferred EXCEPT ![s] = Defer]
    /\ spc' = [spc EXCEPT ![s] = IF Protocol = "CheckFirst" THEN "checked" ELSE "left"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, stok, sepoch, skeep, sreleased>>

\* Not with `CheckFirst`. A successful re-check finds what a span now names; its restore lands later, alone.
RecheckSucceeds(s) ==
    /\ spc[s] = "left"
    /\ spc' = [spc EXCEPT ![s] = IF sreleased[s] /\ Referenced THEN "restoring" ELSE "checked"]
    /\ UNCHANGED <<live, named, queue, collected, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, stok, sepoch, skeep, sreleased,
                   sdeferred>>

\* Alone with `ReleaseFirst`; with `IntentFirst`, in the transaction that keeps the cleanup owed again.
Restore(s) ==
    /\ spc[s] = "restoring"
    /\ assoc' = TRUE /\ durable' = TRUE
    /\ IF Protocol = "IntentFirst"
          THEN next <= MaxToken /\ owed' = next /\ next' = next + 1
          ELSE UNCHANGED <<owed, next>>
    /\ spc' = [spc EXCEPT ![s] = "checked"]
    /\ UNCHANGED <<live, named, queue, collected, pending, stamped, seq, wpc, wstamp, stok, sepoch, skeep, sreleased, sdeferred>>

\* A failed re-check: the compensation restores what it released and keeps the cleanup owed, in one transaction -
\* which commits, or fails and changes nothing. Either way the pass returns an error.
RecheckFails(s) ==
    /\ spc[s] = "left" /\ sreleased[s] /\ next <= MaxToken
    /\ \/ /\ assoc' = TRUE /\ durable' = TRUE /\ owed' = next /\ next' = next + 1
       \/ UNCHANGED <<assoc, durable, owed, next>>
    /\ spc' = [spc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<live, named, queue, collected, pending, stamped, seq, wpc, wstamp, stok, sepoch, skeep, sreleased, sdeferred>>

\* Success. A sweep completes the cleanup only while the row carries the token it claimed, and not if it deferred. A
\* pass keeps a deferred trace's cleanup owed under a fresh token, then clears the entries it read.
Finish(s) ==
    /\ spc[s] = "checked"
    /\ IF s \in Sweeps
          THEN /\ owed' = IF ~sdeferred[s] /\ {owed} = stok[s] THEN 0 ELSE owed
               /\ UNCHANGED <<queue, next>>
          ELSE /\ IF sdeferred[s]
                     THEN next <= MaxToken /\ owed' = next /\ next' = next + 1
                     ELSE UNCHANGED <<owed, next>>
               /\ queue' = queue \ stok[s]
    /\ spc' = [spc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<live, named, collected, assoc, durable, pending, stamped, seq, wpc, wstamp, stok, sepoch, skeep, sreleased,
                   sdeferred>>

\* The orphan sweep: a file no trace is associated with has its bytes reclaimed, at any moment.
Collect ==
    /\ ~assoc /\ ~collected
    /\ collected' = TRUE
    /\ UNCHANGED <<live, named, queue, assoc, durable, pending, stamped, seq, owed, next, wpc, wstamp, sweepVars>>

Next ==
    \/ Collect
    \/ \E w \in Writers : Associate(w) \/ WriteSpan(w) \/ Draw(w) \/ Confirm(w) \/ Abort(w)
    \/ \E sp \in Spans : Delete(sp)
    \/ Rewrite
    \/ \E s \in Reconcilers : Claim(s) \/ Scan(s) \/ Intend(s) \/ Release(s) \/ Left(s) \/ RecheckSucceeds(s) \/ Restore(s)
                         \/ RecheckFails(s) \/ Finish(s)

Spec == Init /\ [][Next]_vars

InFlight(s) == spc[s] # "idle"

\* A file a committed span names stays associated - but for a `ReleaseFirst` sweep between its release and its
\* restore, the window its re-check exists to close.
LiveFileKept ==
    Referenced => (assoc \/ \E s \in Reconcilers : sreleased[s] /\ spc[s] \in {"released", "left", "restoring"})

\* No committed span names a file whose bytes were reclaimed: acknowledged media is never lost.
NoMediaLost == ~(Referenced /\ collected)

\* A file associated and referenced by nothing is still owed a reconciliation.
OwedNotLost ==
    (assoc /\ ~Referenced) => (owed # 0 \/ queue # {} \/ pending > 0 \/ \E s \in Reconcilers : InFlight(s))

Symmetry == Permutations(Writers) \cup Permutations(Sweeps) \cup Permutations(Passes)
=============================================================================
