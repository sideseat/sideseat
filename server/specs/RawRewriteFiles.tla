---------------------------- MODULE RawRewriteFiles ----------------------------
(***************************************************************************)
(* A raw rewrite and the files of the spans it removes.                    *)
(*                                                                         *)
(* One trace's raw record names spans, each carrying a file the trace      *)
(* is associated with. A span's deletion removes its row and queues the    *)
(* record; the record still names the span, which keeps its file. A        *)
(* rewrite reads the latest version - its key and its bytes - and prepares *)
(* it without what the deletions so far refuse, keyed above it; it can be  *)
(* given a later key still, as an epoch read after another collection      *)
(* gives one. A repair can land a version at the key it read, holding what *)
(* the latest holds without the deleted spans. Appending queues the record *)
(* in the same step, as DuckDB's `append_raw_rewrites` does.               *)
(*                                                                         *)
(* A file association is released by a reconciliation, which keeps what    *)
(* the latest version and the live rows reference. With `Derived` FALSE    *)
(* the rewrite reconciles right after its append, best effort; with it     *)
(* TRUE the reconciler pass its append queued reconciles, before clearing  *)
(* the entries it read. Any step can crash.                                *)
(*                                                                         *)
(* `BasedOnCheck` admits a rewrite only while the version it was prepared  *)
(* from is still the latest. Without it, or with `KeyOnly` - the version   *)
(* told apart by its key alone - `LatestMediaHeld` breaks: a rewrite       *)
(* prepared before a newer one landed brings back a span whose file the    *)
(* newer one's reconciliation released.                                    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Spans,         \* the record's spans, each carrying a file of its own
          Rewriters, Passes,
          MaxKey,        \* bound on version keys and queue entries, for the model checker
          Derived,       \* the pass the append queued reconciles, rather than the rewrite itself
          BasedOnCheck,  \* a rewrite lands only while the version it was prepared from is the latest
          KeyOnly,       \* that version is told apart by its key alone
          AtomicQueue,   \* the append and its queueing are one step
          ClearRead      \* a pass clears only the entries it read

ASSUME MaxKey \in Nat /\ Derived \in BOOLEAN /\ BasedOnCheck \in BOOLEAN /\ KeyOnly \in BOOLEAN
ASSUME AtomicQueue \in BOOLEAN /\ ClearRead \in BOOLEAN

\* A span and the file it carries share a name.
Files == Spans
SpanOf(f) == f

VARIABLES
    latest,   \* the latest version: [key, content], content the spans it names
    deleted,  \* the spans deleted so far: their rows gone, refused by every rewrite
    live,     \* live[f]: a span of the trace ingested again names f
    assoc,    \* assoc[f]: the trace is associated with f
    queue,    \* the record's queued entries, by token
    next,     \* the next entry token
    wpc,      \* each rewriter's step
    wbase,    \* the version each rewriter read
    wnew,     \* the version each rewriter will append
    ppc, pread \* each pass's step, and the entries it read

vars == <<latest, deleted, live, assoc, queue, next, wpc, wbase, wnew, ppc, pread>>

Version == [key : 0..MaxKey, content : SUBSET Spans]

Referenced(f) == SpanOf(f) \in latest.content \/ live[f]
Reconciled == [f \in Files |-> assoc[f] /\ Referenced(f)]

\* Whether `v` is still the version a rewrite read as `base`.
Same(v, base) == IF KeyOnly THEN v.key = base.key ELSE v = base

TypeOK ==
    /\ latest \in Version /\ deleted \subseteq Spans
    /\ live \in [Files -> BOOLEAN] /\ assoc \in [Files -> BOOLEAN]
    /\ queue \subseteq 1..MaxKey /\ next \in 1..(MaxKey + 1)
    /\ wpc \in [Rewriters -> {"idle", "read", "appended", "landed", "done", "crashed"}]
    /\ ppc \in [Passes -> {"idle", "read", "reconciled"}]

Init ==
    /\ latest = [key |-> 0, content |-> Spans] /\ deleted = {}
    /\ live = [f \in Files |-> FALSE] /\ assoc = [f \in Files |-> TRUE]
    /\ queue = {} /\ next = 1
    /\ wpc = [w \in Rewriters |-> "idle"]
    /\ wbase = [w \in Rewriters |-> latest] /\ wnew = [w \in Rewriters |-> latest]
    /\ ppc = [p \in Passes |-> "idle"] /\ pread = [p \in Passes |-> {}]

Enqueue == queue' = queue \cup {next} /\ next' = next + 1

Delete(s) ==
    /\ s \notin deleted /\ next <= MaxKey
    /\ deleted' = deleted \cup {s}
    /\ Enqueue
    /\ UNCHANGED <<latest, live, assoc, wpc, wbase, wnew, ppc, pread>>

Reingest(f) ==
    /\ ~live[f]
    /\ live' = [live EXCEPT ![f] = TRUE]
    /\ assoc' = [assoc EXCEPT ![f] = TRUE]
    /\ UNCHANGED <<latest, deleted, queue, next, wpc, wbase, wnew, ppc, pread>>

\* A repair keyed at the latest's own key: what the latest holds, without the deleted spans. Queued as it lands.
Repair ==
    /\ latest.content \cap deleted # {} /\ next <= MaxKey
    /\ latest' = [key |-> latest.key, content |-> latest.content \ deleted]
    /\ Enqueue
    /\ UNCHANGED <<deleted, live, assoc, wpc, wbase, wnew, ppc, pread>>

\* A rewrite reads the latest version and prepares it without what the deletions so far refuse, keyed above it -
\* or above that, as a later epoch keys it.
RewriteRead(w) ==
    /\ wpc[w] = "idle" /\ latest.content \cap deleted # {}
    /\ \E key \in (latest.key + 1)..MaxKey :
          wnew' = [wnew EXCEPT ![w] = [key |-> key, content |-> latest.content \ deleted]]
    /\ wbase' = [wbase EXCEPT ![w] = latest]
    /\ wpc' = [wpc EXCEPT ![w] = "read"]
    /\ UNCHANGED <<latest, deleted, live, assoc, queue, next, ppc, pread>>

\* The append: refused unless the version it was prepared from is still the latest, with `BasedOnCheck`; otherwise,
\* the prepared version lands however late, as the latest if no key above it has - and with `AtomicQueue`, queued
\* in the same step.
Append(w) ==
    /\ wpc[w] = "read" /\ next <= MaxKey
    /\ IF BasedOnCheck /\ ~Same(latest, wbase[w])
          THEN /\ wpc' = [wpc EXCEPT ![w] = "done"]
               /\ UNCHANGED <<latest, queue, next>>
          ELSE /\ latest' = IF wnew[w].key >= latest.key THEN wnew[w] ELSE latest
               /\ IF AtomicQueue
                     THEN Enqueue /\ wpc' = [wpc EXCEPT ![w] = "landed"]
                     ELSE UNCHANGED <<queue, next>> /\ wpc' = [wpc EXCEPT ![w] = "appended"]
    /\ UNCHANGED <<deleted, live, assoc, wbase, wnew, ppc, pread>>

QueueAfter(w) ==
    /\ wpc[w] = "appended" /\ next <= MaxKey
    /\ Enqueue
    /\ wpc' = [wpc EXCEPT ![w] = "landed"]
    /\ UNCHANGED <<latest, deleted, live, assoc, wbase, wnew, ppc, pread>>

\* The rewrite's own reconciliation after its append, without `Derived` - which can fail and change nothing.
InlineReconcile(w) ==
    /\ wpc[w] = "landed"
    /\ assoc' \in IF Derived THEN {assoc} ELSE {Reconciled, assoc}
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<latest, deleted, live, queue, next, wbase, wnew, ppc, pread>>

RewriteCrash(w) ==
    /\ wpc[w] \in {"read", "appended", "landed"}
    /\ wpc' = [wpc EXCEPT ![w] = "crashed"]
    /\ UNCHANGED <<latest, deleted, live, assoc, queue, next, wbase, wnew, ppc, pread>>

PassRead(p) ==
    /\ ppc[p] = "idle" /\ queue # {}
    /\ pread' = [pread EXCEPT ![p] = queue]
    /\ ppc' = [ppc EXCEPT ![p] = "read"]
    /\ UNCHANGED <<latest, deleted, live, assoc, queue, next, wpc, wbase, wnew>>

\* With `Derived`, the pass reconciles the record's traces' files; without, it reconciles nothing.
PassReconcile(p) ==
    /\ ppc[p] = "read"
    /\ assoc' = IF Derived THEN Reconciled ELSE assoc
    /\ ppc' = [ppc EXCEPT ![p] = "reconciled"]
    /\ UNCHANGED <<latest, deleted, live, queue, next, wpc, wbase, wnew, pread>>

PassClear(p) ==
    /\ ppc[p] = "reconciled"
    /\ queue' = IF ClearRead THEN queue \ pread[p] ELSE {}
    /\ ppc' = [ppc EXCEPT ![p] = "idle"]
    /\ UNCHANGED <<latest, deleted, live, assoc, next, wpc, wbase, wnew, pread>>

PassCrash(p) ==
    /\ ppc[p] \in {"read", "reconciled"}
    /\ ppc' = [ppc EXCEPT ![p] = "idle"]
    /\ UNCHANGED <<latest, deleted, live, assoc, queue, next, wpc, wbase, wnew, pread>>

Next ==
    \/ \E s \in Spans : Delete(s)
    \/ \E f \in Files : Reingest(f)
    \/ Repair
    \/ \E w \in Rewriters :
          RewriteRead(w) \/ Append(w) \/ QueueAfter(w) \/ InlineReconcile(w) \/ RewriteCrash(w)
    \/ \E p \in Passes : PassRead(p) \/ PassReconcile(p) \/ PassClear(p) \/ PassCrash(p)

Spec == Init /\ [][Next]_vars

\* The latest version names only files its trace holds: a file released and still named is a record that cannot
\* be decoded once the file's bytes are collected.
LatestMediaHeld == \A f \in Files : SpanOf(f) \in latest.content => assoc[f]

\* A file a live row names stays associated.
LiveFileKept == \A f \in Files : live[f] => assoc[f]

\* With `Derived`: a file associated and referenced by nothing is always still owed a reconciliation - a queued
\* entry, a pass that has read and not reconciled, a rewrite that will still append and queue.
StillOwed ==
    \/ queue # {}
    \/ \E p \in Passes : ppc[p] = "read"
    \/ \E w \in Rewriters : wpc[w] = "read" \/ (wpc[w] = "appended" /\ ~AtomicQueue)
OwedNotLost == Derived => \A f \in Files : (assoc[f] /\ ~Referenced(f)) => StillOwed

Symmetry == Permutations(Rewriters) \cup Permutations(Passes)
=============================================================================
