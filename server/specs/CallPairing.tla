---------------------------- MODULE CallPairing ----------------------------
(***************************************************************************)
(* Tool call/result pairing decided once, at the source: one execution     *)
(* table per trace, built before dedup, that every later step reads. Its   *)
(* cases, one per review finding, are the domain feed's pairing tests.     *)
(*                                                                         *)
(* The ground truth is a trace's executions, each a tool call made at its  *)
(* own time, with an id or none, a shape (tool and arguments), and perhaps *)
(* a result emitted at or after the call: on the executing span's own      *)
(* record, which pairs it by place, or elsewhere, which pairs it by id or, *)
(* without one, by tool. Every call emission creates its execution. A      *)
(* request recorded at time `T` carries copies: calls it re-sends, and a   *)
(* result it restates, perhaps without its id. A copy never creates an     *)
(* execution and never answers one.                                        *)
(*                                                                         *)
(* A call copy binds to the executions before `T` with its id and shape:   *)
(* the only one, or the k-th where the payload holds as many such copies   *)
(* as there are executions, k being the copy's rank among them. Otherwise  *)
(* it is bound to nothing and keeps its own identity, which may show a     *)
(* duplicate but never hides an execution. A result emission off its      *)
(* span binds to the one unanswered execution it could answer, or to none  *)
(* when two could. A result copy binds to the one emission it restates.    *)
(*                                                                         *)
(* Properties: `Conservation` (an execution is shown once, more only       *)
(* where a copy of it is genuinely ambiguous), `CopiesCreateNothing`,      *)
(* `Matching` (a result binds to its own execution or to none, and to its  *)
(* own whenever no other execution could take it), `ShapeSafety`,          *)
(* `DeliveryInvariance` (copies change no binding) and `FailureAccounting` *)
(* (a failure counts as answered only by a result of its own execution).   *)
(* Each mutant flag replaces one rule with what the passes it supersedes   *)
(* did, and TLC finds the property it breaks.                              *)
(*                                                                         *)
(* Not modelled: payload classification (which carrier holds an emission   *)
(* and which a copy, the carrier facts' job, checked by the goldens), and  *)
(* executions outside the trace, which a copy shows as its own.            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    NE,             \* executions in the trace, called at times 1..NE
    RankAmongShape, \* a call copy's rank counts every same-shaped copy, whatever its id
    NoShapeCheck,   \* a reused id binds a call copy whatever its shape
    FirstCandidate, \* an ambiguous result binds to the oldest candidate instead of none
    CopiesConsume   \* a result copy answers an execution, as an emission does

\* The time slots of `Order` are spelled out for three executions.
ASSUME NE = 3

Exec == 1..NE
NoId == "none"
Ids == {"x", "y"}
Shapes == {1, 2}
Contents == {1, 2}
None == 0

VARIABLES
    id,       \* each execution's call id, or NoId
    shape,    \* each execution's tool and arguments
    answered, \* a result was emitted for the execution
    onSpan,   \* that result is on the executing span's own record
    rtime,    \* when the result was emitted
    content,  \* what the result says
    T,        \* when the request carrying the copies was recorded
    cc,       \* the request's call copies: the execution each restates
    rc,       \* the request's result copies: the execution whose result each restates, and whether its id was dropped
    final,    \* the matching, computed once per state
    alone     \* the matching without the request's copies

vars == <<id, shape, answered, onSpan, rtime, content, T, cc, rc, final, alone>>

SeqsUpTo(S, n) == UNION {[1..k -> S] : k \in 0..n}
Min(S) == CHOOSE e \in S : \A f \in S : e <= f
Kth(S, k) == CHOOSE e \in S : Cardinality({f \in S : f < e}) = k - 1

(* Call copies. *)

SameCall(e, f) == id[e] = id[f] /\ (shape[e] = shape[f] \/ (NoShapeCheck /\ id[e] # NoId))
Class(j) == {f \in Exec : f < T /\ SameCall(cc[j], f)}
Alike(j) == {i \in DOMAIN cc : SameCall(cc[i], cc[j])}
Rank(j) == Cardinality({i \in Alike(j) : i < j})
ShapeRank(j) == Cardinality({i \in DOMAIN cc : i < j /\ shape[cc[i]] = shape[cc[j]]})

CallBind(j) ==
    LET C == Class(j)
        n == Cardinality(C)
    IN  IF RankAmongShape THEN (IF ShapeRank(j) < n THEN Kth(C, ShapeRank(j) + 1) ELSE None)
        ELSE IF n = 1 THEN Min(C)
        ELSE IF n = Cardinality(Alike(j)) THEN Kth(C, Rank(j) + 1)
        ELSE None

\* What a reader can tell about a copy, whatever the rules: the executions it could restate.
Restatable(j) == {f \in Exec : f < T /\ id[f] = id[cc[j]] /\ shape[f] = shape[cc[j]]}
Genuinely(j) ==
    /\ Cardinality(Restatable(j)) >= 2
    /\ Cardinality(Restatable(j)) # Cardinality({i \in DOMAIN cc : Restatable(i) = Restatable(j)})

Shown(e) == 1 + Cardinality({j \in DOMAIN cc : cc[j] = e /\ CallBind(j) = None})

(* Result emissions and copies, matched to executions. *)

Emitted == {r \in Exec : answered[r]}
ByPlace == {r \in Emitted : onSpan[r]}
Cands(oid, tool, tau, taken) ==
    {e \in Exec \ taken : e <= tau /\ id[e] = oid /\ shape[e] = tool}
Pick(S) == IF Cardinality(S) = 1 THEN Min(S) ELSE IF FirstCandidate /\ S # {} THEN Min(S) ELSE None

\* One step of the matching: an emission `r`, or the request's result copy when `r` is None.
Take(prev, r) ==
    LET e == IF r # None THEN Pick(Cands(id[r], shape[r], rtime[r], prev.taken))
             ELSE Pick(Cands(IF rc[1].anon THEN NoId ELSE id[rc[1].of], shape[rc[1].of], T - 1, prev.taken))
    IN  [taken |-> IF e = None THEN prev.taken ELSE prev.taken \cup {e},
         bind |-> IF r # None THEN [prev.bind EXCEPT ![r] = e] ELSE prev.bind]

\* The events in time order: the off-span emissions by time and then execution, the request's copy - an event only
\* where copies answer - just before the emissions of its own time.
Order(useCopies) ==
    LET copy == IF useCopies /\ CopiesConsume /\ rc # <<>> THEN <<None>> ELSE <<>>
        at(t) == [i \in 1..Cardinality({r \in Emitted \ ByPlace : rtime[r] = t}) |->
                    Kth({r \in Emitted \ ByPlace : rtime[r] = t}, i)]
        slot(t) == IF t = T THEN copy \o at(t) ELSE at(t)
    IN  slot(1) \o slot(2) \o slot(3) \o slot(4)

Acc(useCopies) ==
    LET events == Order(useCopies)
        step[k \in 0..Len(events)] ==
            IF k = 0 THEN [taken |-> ByPlace, bind |-> [r \in Exec |-> IF r \in ByPlace THEN r ELSE None]]
            ELSE Take(step[k - 1], events[k])
    IN  step[Len(events)]

Bind(r) == final.bind[r]

ResultCopyBind(c) ==
    LET Q == {q \in Emitted : rtime[q] < T /\ content[q] = content[c.of] /\ shape[q] = shape[c.of]
                              /\ (c.anon \/ id[q] = id[c.of])}
    IN  IF Cardinality(Q) = 1 THEN Min(Q) ELSE None

(* The model. *)

Init ==
    /\ id \in {f \in [Exec -> Ids \cup {NoId}] : f[1] # "y"}
    /\ shape \in {f \in [Exec -> Shapes] : f[1] = 1}
    /\ answered \in [Exec -> BOOLEAN]
    \* What an unanswered execution's result would have been is no observable.
    /\ onSpan \in {f \in [Exec -> BOOLEAN] : \A e \in Exec : f[e] => answered[e]}
    /\ rtime \in {f \in [Exec -> Exec] : \A e \in Exec : f[e] >= e /\ (~answered[e] => f[e] = e)}
    /\ content \in {f \in [Exec -> Contents] : f[1] = 1 /\ \A e \in Exec : ~answered[e] => f[e] = 1}
    /\ T \in 1..(NE + 1)
    /\ cc \in SeqsUpTo({e \in Exec : e < T}, 2)
    /\ rc \in SeqsUpTo([of : {r \in Emitted : rtime[r] < T}, anon : BOOLEAN], 1)
    /\ final = Acc(TRUE)
    /\ alone = Acc(FALSE)

Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars

(* Properties. *)

TypeOK ==
    /\ \A j \in DOMAIN cc : CallBind(j) \in Exec \cup {None}
    /\ \A r \in Exec : Bind(r) \in Exec \cup {None}

\* Every execution is shown by its emission; a copy shows it again only where the copy could restate either of two
\* executions alike in every observable.
Conservation ==
    \A e \in Exec : Shown(e) > 1 => \E j \in DOMAIN cc : cc[j] = e /\ CallBind(j) = None /\ Genuinely(j)

\* A bound copy is bound to an execution it could restate: its id, called before the request.
CopiesCreateNothing ==
    /\ \A j \in DOMAIN cc : CallBind(j) # None => id[CallBind(j)] = id[cc[j]] /\ CallBind(j) < T
    /\ \A i \in DOMAIN rc : ResultCopyBind(rc[i]) # None =>
           /\ content[ResultCopyBind(rc[i])] = content[rc[i].of]
           /\ (rc[i].anon \/ id[ResultCopyBind(rc[i])] = id[rc[i].of])
    /\ \A i \in DOMAIN rc :
           {q \in Emitted : rtime[q] < T /\ content[q] = content[rc[i].of] /\ shape[q] = shape[rc[i].of]
                            /\ (rc[i].anon \/ id[q] = id[rc[i].of])} = {rc[i].of}
               => ResultCopyBind(rc[i]) = rc[i].of

\* A result binds to its own execution or to none - so no two results share one - and to its own whenever no other
\* execution it could answer is left unanswered by place.
Matching ==
    \A r \in Emitted :
        /\ Bind(r) \in {None, r}
        /\ {e \in Exec \ ByPlace : e # r /\ e <= rtime[r] /\ id[e] = id[r] /\ shape[e] = shape[r]} = {}
               => Bind(r) = r

\* No copy and no result binds across shapes.
ShapeSafety ==
    /\ \A j \in DOMAIN cc : CallBind(j) # None => shape[CallBind(j)] = shape[cc[j]]
    /\ \A r \in Emitted : Bind(r) # None => shape[Bind(r)] = shape[r]

\* The request's copies change no result's binding and answer nothing.
DeliveryInvariance == final = alone

\* A failure is taken as answered only where a result of its own execution was emitted.
FailureAccounting == \A e \in final.taken : answered[e]

=============================================================================
