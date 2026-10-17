-------------------------- MODULE SessionMembership --------------------------
(***************************************************************************)
(* One trace, its spans written by instances that run concurrently, and a   *)
(* deleted session D: when may D's deletion take the trace.                 *)
(*                                                                          *)
(* A trace is its root's session - a root being a span with no parent that  *)
(* names one - else the session of its earliest span that names one         *)
(* (`docs/engineering/write-protocol.md`, section 6). A root can arrive     *)
(* after its children, so a trace with no root stored may yet be moved: it  *)
(* waits for its root until `Grace` has passed since its first span was     *)
(* stored, and a root stored after that is dropped with the trace.          *)
(*                                                                          *)
(* A writer passes the pre-write fence, stores its span, then checks - the  *)
(* post-write session check, and the trace-tombstone check that compensates *)
(* a span written under a tombstone - and is acknowledged. The fence takes  *)
(* the trace only on a root: one it carries, or one stored. The post-write  *)
(* check takes it only once a root is stored. A deleter - the session       *)
(* sweep, the deletion request and a restore's replay decide alike - takes  *)
(* its cutoff, reads the trace's membership, decides, and in a later step   *)
(* tombstones it; the trace sweep then deletes a tombstoned trace's rows.   *)
(* Nothing orders the deleter's read and its tombstone against the writers: *)
(* a rootless trace taken only once the grace has passed by a cutoff taken  *)
(* before the read is what makes a root landing in between a late one.      *)
(*                                                                          *)
(* `LiveRootKept`: a root naming a live session is never dropped or deleted *)
(* unless it was stored after the grace. Each guard is necessary: without   *)
(* `RootFirst` (the earliest span decides), `DecidingRoot` (a root naming   *)
(* no session taken as deciding), `Gate`, `CutoffFirst` (the deadline       *)
(* compared with the time the read returned, which a chunked read can carry *)
(* past it), `FenceOnRoot` or `PostWriteRooted`, TLC finds a live root lost *)
(* before the grace.                                                        *)
(*                                                                          *)
(* Modelled as one atomic step each: a writer's fence and store, and its    *)
(* two checks with its acknowledgement; the deleter's read. Every store is  *)
(* visible to every later read - on ClickHouse the membership read is of    *)
(* every replica, so a root committed on any of them is seen.               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Grace,      \* ticks a rootless trace waits for its root
    MaxTime,    \* the clock's bound
    RootFirst,  \* the root's session decides; FALSE: the earliest span's
    DecidingRoot,\* only a root naming a session decides; FALSE: any parentless span counts as the root
    Gate,       \* the sweep holds a rootless trace until the grace passes
    CutoffFirst,\* the grace's deadline is taken before the read, not when it returns
    FenceOnRoot,\* the pre-write fence takes the trace only on a root
    PostWriteRooted \* the post-write check takes the trace only when a deciding root is stored

VARIABLES
    now,        \* the clock
    stored,     \* writers whose span is stored and not deleted
    at,         \* when each writer's span was stored (0: not yet)
    pc,         \* each writer's step: "idle", "written", "done"
    lost,       \* ghost: writers whose span was dropped at a fence, compensated, or deleted
    tomb,       \* the trace is tombstoned
    first,      \* when the trace's first span was stored (0: none yet)
    sweep,      \* the session sweep: its step, and what its read decided
    dropAt      \* ghost: when a fence dropped each writer's span (0: not dropped there)

vars == <<now, stored, at, pc, lost, tomb, first, sweep, dropAt>>

\* Two children naming the deleted session; a root naming a live session that starts after both, so only the
\* root-first rule puts the trace in the live session; and a root naming no session, which decides nothing.
Writers == {"c1", "c2", "r", "n"}
Span == [w \in Writers |->
            CASE w = "c1" -> [root |-> FALSE, session |-> "D", start |-> 2]
              [] w = "c2" -> [root |-> FALSE, session |-> "D", start |-> 3]
              [] w = "r"  -> [root |-> TRUE,  session |-> "L", start |-> 4]
              [] w = "n"  -> [root |-> TRUE,  session |-> "", start |-> 1]]
Deleted == {"D"}

Earliest(S) == CHOOSE w \in S : \A v \in S : Span[w].start <= Span[v].start
Named(S) == {w \in S : Span[w].session # ""}
\* The roots that decide: a root naming a session.
Roots(S) == {w \in Named(S) : Span[w].root}
Member(S) ==
    IF Named(S) = {} THEN "none"
    ELSE IF RootFirst /\ Roots(S) # {} THEN Span[Earliest(Roots(S))].session
    ELSE Span[Earliest(Named(S))].session
\* What the deleters and fences take as "the root is stored": without `DecidingRoot`, any parentless span.
Rooted(S) == IF DecidingRoot THEN Roots(S) # {} ELSE \E w \in S : Span[w].root

\* The deleter at rest.
Idle == [step |-> "idle", cutoff |-> 0, rooted |-> FALSE, eligible |-> FALSE]


TypeOK ==
    /\ now \in 1..MaxTime
    /\ stored \subseteq Writers
    /\ pc \in [Writers -> {"idle", "written", "done"}]
    /\ lost \subseteq Writers
    /\ tomb \in BOOLEAN
    /\ sweep.step \in {"idle", "sampled", "read", "decided"}

Init ==
    /\ now = 1
    /\ stored = {}
    /\ at = [w \in Writers |-> 0]
    /\ pc = [w \in Writers |-> "idle"]
    /\ lost = {}
    /\ tomb = FALSE
    /\ first = 0
    /\ sweep = Idle
    /\ dropAt = [w \in Writers |-> 0]

\* The pre-write fence. A tombstoned trace drops the span. A trace whose stored root names a deleted session, or a
\* span that is itself a root naming one for a trace with no stored root, is fenced - tombstoned and dropped.
\* Without `FenceOnRoot` the store's answer is taken for a rootless trace too.
Fenced(w) ==
    \/ /\ Rooted(stored)
       /\ Member(stored) \in Deleted
    \/ /\ ~Rooted(stored)
       /\ Span[w].root
       /\ Span[w].session \in Deleted
    \/ /\ ~FenceOnRoot
       /\ stored # {}
       /\ ~Rooted(stored)
       /\ Member(stored \cup {w}) \in Deleted

Write(w) ==
    /\ pc[w] = "idle"
    /\ IF tomb \/ Fenced(w)
       THEN /\ tomb' = TRUE
            /\ lost' = lost \cup {w}
            /\ dropAt' = [dropAt EXCEPT ![w] = now]
            /\ pc' = [pc EXCEPT ![w] = "done"]
            /\ UNCHANGED <<stored, at, first>>
       ELSE /\ stored' = stored \cup {w}
            /\ at' = [at EXCEPT ![w] = now]
            /\ first' = IF first = 0 THEN now ELSE first
            /\ pc' = [pc EXCEPT ![w] = "written"]
            /\ UNCHANGED <<lost, tomb, dropAt>>
    /\ UNCHANGED <<now, sweep>>

\* The post-write checks, then the acknowledgement: the session check tombstones the trace if a deciding root is
\* stored and puts it in a deleted session - without `PostWriteRooted`, on whatever the store answers - and the
\* trace check compensates a span written under a tombstone. (The code also checks only traces its batch began, which
\* bounds the reads; the model shows the stored root is what makes the decision safe.)
Check(w) ==
    /\ pc[w] = "written"
    /\ LET doomed == /\ PostWriteRooted => Rooted(stored)
                     /\ Member(stored) \in Deleted
           t == tomb \/ doomed
       IN /\ tomb' = t
          /\ IF t /\ w \in stored
             THEN /\ stored' = stored \ {w}
                  /\ lost' = lost \cup {w}
             ELSE UNCHANGED <<stored, lost>>
    /\ pc' = [pc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<now, at, first, sweep, dropAt>>

\* The trace sweep deletes a tombstoned trace's rows.
TraceSweep ==
    /\ tomb
    /\ stored # {}
    /\ lost' = lost \cup stored
    /\ stored' = {}
    /\ UNCHANGED <<now, at, pc, tomb, first, sweep, dropAt>>

\* The deleter takes its cutoff - the time the grace is measured against.
SweepSample ==
    /\ sweep.step = "idle"
    /\ sweep' = [Idle EXCEPT !.step = "sampled", !.cutoff = now]
    /\ UNCHANGED <<now, stored, at, pc, lost, tomb, first, dropAt>>

\* Its read: the trace in a deleted session, and whether its root is stored.
SweepRead ==
    /\ sweep.step = "sampled"
    /\ stored # {}
    /\ Member(stored) \in Deleted
    /\ sweep' = [sweep EXCEPT !.step = "read", !.rooted = Rooted(stored)]
    /\ UNCHANGED <<now, stored, at, pc, lost, tomb, first, dropAt>>

\* Its decision once the read has returned: eligible if the root is stored, or the grace has passed - by the cutoff
\* taken before the read, or without `CutoffFirst` by the time now. Without `Gate`, eligible regardless.
SweepDecide ==
    /\ sweep.step = "read"
    /\ sweep' = [sweep EXCEPT !.step = "decided",
                  !.eligible = \/ ~Gate
                               \/ sweep.rooted
                               \/ first + Grace <= (IF CutoffFirst THEN sweep.cutoff ELSE now)]
    /\ UNCHANGED <<now, stored, at, pc, lost, tomb, first, dropAt>>

\* Its tombstone, on what it decided.
SweepApply ==
    /\ sweep.step = "decided"
    /\ tomb' = (tomb \/ sweep.eligible)
    /\ sweep' = Idle
    /\ UNCHANGED <<now, stored, at, pc, lost, first, dropAt>>

\* A read that finds nothing to decide ends the pass.
SweepQuiet ==
    /\ sweep.step = "sampled"
    /\ ~(stored # {} /\ Member(stored) \in Deleted)
    /\ sweep' = Idle
    /\ UNCHANGED <<now, stored, at, pc, lost, tomb, first, dropAt>>

Tick ==
    /\ now < MaxTime
    /\ now' = now + 1
    /\ UNCHANGED <<stored, at, pc, lost, tomb, first, sweep, dropAt>>

Next ==
    \/ \E w \in Writers : Write(w) \/ Check(w)
    \/ TraceSweep
    \/ SweepSample
    \/ SweepRead
    \/ SweepDecide
    \/ SweepApply
    \/ SweepQuiet
    \/ Tick

Spec == Init /\ [][Next]_vars

\* A root naming a live session is lost only if it was stored after the grace - the late root the decision accepts.
\* A root dropped at a fence was never stored; it counts as arriving when it was dropped.
LiveRootKept ==
    \A w \in Writers :
        (Span[w].root /\ Span[w].session \notin Deleted \cup {""} /\ w \in lost)
            => (first # 0 /\ (IF at[w] # 0 THEN at[w] ELSE dropAt[w]) >= first + Grace)

\* A deleted session takes only its own: nothing is lost from a trace whose stored root names a live session.
NoLossUnderLiveRoot ==
    (RootFirst /\ Rooted(stored) /\ Member(stored) \notin Deleted /\ ~tomb)
        => \A w \in stored : w \notin lost

=============================================================================
