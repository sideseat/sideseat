---------------------------- MODULE OrderGraph ----------------------------
(***************************************************************************)
(* The message-ordering model of the SideML reconstruction pipeline.        *)
(*                                                                         *)
(* Reconstruction sees many *observations* of one logical message: a        *)
(* generation span emits it, a parent chain span re-lists it as accumulated *)
(* state, a later span re-sends it as history. Dedup collapses observations *)
(* to one surviving block each; ordering then has to place those survivors. *)
(*                                                                         *)
(* This specifies server/src/domain/sideml/feed/order_graph.rs. It exists   *)
(* because the ordering rules interact: six candidate scalar-anchor designs *)
(* were each rejected after breaking a different framework, and every       *)
(* failure was found only by diffing 111 golden fixtures. Here the rules    *)
(* are stated once and TLC checks that they cannot contradict each other -  *)
(* over every input shape in the bounded model, not over the shapes the     *)
(* corpus happens to contain.                                              *)
(*                                                                         *)
(* Modelled: observations and their carriers, the dedup lineage onto        *)
(* survivors, emission contraction as a quotient, the three edge classes,   *)
(* and resolution as deterministic Kahn with a total cycle fallback.        *)
(*                                                                         *)
(* Not modelled, and stated because the distinction matters: content,       *)
(* hashing, quality scoring, history marking. Which observation survives is *)
(* dedup's business.                                                        *)
(*                                                                          *)
(* The spec checks each lineage independently, so it proves the properties  *)
(* below hold for *every* survivor selection. It does NOT prove that two    *)
(* equivalent selections yield the same order - that needs two lineages     *)
(* compared in one state, which squares the space, and `Rank` is fixed here *)
(* besides. Copy-survival independence is therefore tested in Rust, over    *)
(* real payloads, by `ordering_constraints_do_not_change_a_session_s_messages`  *)
(* and the promoted-constraint tests, not claimed here.                     *)
(*                                                                          *)
(* Also outside the model: the promotion dial, time priority, the           *)
(* history gating that differs between carrier and dataflow edges, and the  *)
(* session transcript. Each is named in `Constraints::PRODUCTION`.          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences, TLC

(* The model's atoms. Everything structural is derived below, because a .cfg    *)
(* file cannot express a record, and the carrier profiles are records.          *)
CONSTANTS ob1, ob2, ob3,        \* observations
          sv1, sv2,             \* survivors
          none,              \* sentinel: an observation that survived nothing
          spA, spB,          \* spans
          p1, p2, p3         \* payload instances

Obs == {ob1, ob2, ob3}
Surv == {sv1, sv2}
NoSurv == none

\* The pre-resolver order: what the previous scalar sort produced, and the
\* deterministic tie-break here.
Rank == (sv1 :> 0) @@ (sv2 :> 1)

(* The carrier shapes, named for what they are in the corpus:                   *)
(*   choice  - gen_ai.choice / ai.response: an emission the span produced       *)
(*   inMsgs  - llm.input_messages / ai.prompt: a snapshot the span received     *)
(*   state   - output.value: accumulated framework state, on a chain span       *)
(* Two spans, because a message emitted by one span and re-listed by another is  *)
(* the case the whole redesign exists for.                                        *)
(*                                                                                *)
(* Two survivors, and the scope that costs: `Cohesion` is vacuous here, because    *)
(* there is no third block that could land between a contracted pair. A three-     *)
(* survivor model was built and does not complete - TLC enumerates initial states  *)
(* before filtering them, and the space projects to hundreds of hours even with     *)
(* `unit` as a constrained variable rather than a recomputed closure. It is not     *)
(* worth the wait: contiguity is guaranteed by construction rather than by search,  *)
(* since the emit loop flattens each unit's members consecutively, and no ordering  *)
(* rule can interleave two units. What TLC *does* prove here - permutation, and     *)
(* every edge class respected whenever the constraints are satisfiable, over every  *)
(* lineage - is the part that is not obvious by inspection.                         *)
Profiles ==
    { [span |-> spA, payload |-> p1, kind |-> "emission",
       ordered |-> TRUE, output |-> TRUE, gen |-> TRUE],
      [span |-> spA, payload |-> p2, kind |-> "snapshot",
       ordered |-> TRUE, output |-> FALSE, gen |-> TRUE],
      [span |-> spB, payload |-> p1, kind |-> "emission",
       ordered |-> TRUE, output |-> TRUE, gen |-> TRUE],
      [span |-> spB, payload |-> p3, kind |-> "state",
       ordered |-> TRUE, output |-> TRUE, gen |-> FALSE] }

(* Positions are bounded to three values, which is all the ordering rules read:      *)
(* before, after, and same.                                                          *)
(*                                                                                   *)
(* The contraction is a constrained *variable* rather than a computed definition. TLA+ has no          *)
(* memoisation, so `UnitOf` - a transitive closure - is recomputed inside every     *)
(* edge predicate inside every quantifier, which makes the cost per *state* rather  *)
(* than per state-space. Two positions still distinguish "before", "after" and      *)
(* "same", which is all the ordering rules read.                                    *)

VARIABLES lineage, prof, posn, answers, unit

vars == <<lineage, prof, posn, answers, unit>>

----------------------------------------------------------------------------
(* Carrier facts, mirroring sideml::carrier::CarrierSemantics.              *)
(*                                                                          *)
(* payload   which payload instance the observation sat in                   *)
(* kind      "emission" | "snapshot" | "state"                               *)
(* ordered   the payload's positions state an order                          *)
(* output    the span produced this, rather than received it                 *)
(* gen       the span is a generation - a model call                         *)
(* span      which span carried it                                           *)

Carrier(o) == prof[o]
SpanOf(o) == Carrier(o).span
PayloadOf(o) == <<Carrier(o).span, Carrier(o).payload>>

\* An atomic emission the span itself produced. Only these contract, and only
\* these are evidence of *when* a message happened: a snapshot's time is when it
\* was assembled, and a received copy's time is when it was handed back - which
\* is why a `gen_ai.tool.message` on a generation span is not evidence.
IsEmission(o) == Carrier(o).kind = "emission" /\ Carrier(o).output

Ordered(o) == Carrier(o).ordered
Projected(o) == lineage[o] # NoSurv
Landed == { o \in Obs : Projected(o) }

----------------------------------------------------------------------------
(* Units: emission contraction as a quotient over survivors.                *)
(*                                                                          *)
(* Contiguity - "nothing may appear between these" - is NOT expressible as a *)
(* pairwise edge in a partial order: a DAG says A < B and can never say      *)
(* "and nothing between". So the blocks of one emission become ONE node and  *)
(* every external edge attaches to its boundary. This is the load-bearing    *)
(* structural decision, and P1 below is what it buys.                        *)

CoEmitted(a, b) ==
    \E o1, o2 \in Landed :
        /\ lineage[o1] = a /\ lineage[o2] = b
        /\ IsEmission(o1) /\ IsEmission(o2)
        /\ PayloadOf(o1) = PayloadOf(o2)

\* Transitive closure of co-emission, as a set of survivors reachable from s.
RECURSIVE Closure(_, _)
Closure(frontier, seen) ==
    IF frontier = {} THEN seen
    ELSE LET next == { b \in Surv :
                         /\ b \notin seen
                         /\ \E a \in frontier : CoEmitted(a, b) \/ CoEmitted(b, a) }
         IN Closure(next, seen \cup next)

Component(s) == Closure({s}, {s})

\* The contraction, as a *variable* the model chooses and `WellFormedUnits` constrains,
\* rather than a definition computed from `CoEmitted`.
\*
\* Semantically identical - the constraint says exactly "co-emitted survivors share a
\* unit, and the representative is the component's lowest rank" - and the difference is
\* that TLA+ has no memoisation, so as a definition the transitive closure was
\* recomputed inside every edge predicate inside every quantifier of Kahn. At three
\* survivors that projected to hundreds of hours; as a variable it is evaluated once.
UnitOf(s) == unit[s]

\* What makes `unit` a contraction of co-emission and nothing else.
WellFormedUnits ==
    /\ \A s \in Surv : unit[s] \in Surv
    /\ \A s \in Surv : unit[unit[s]] = unit[s]
    \* Co-emitted survivors share a unit ...
    /\ \A a, b \in Surv : CoEmitted(a, b) => unit[a] = unit[b]
    \* ... and nothing else does: a unit's members are exactly one co-emission component.
    /\ \A a, b \in Surv : unit[a] = unit[b] => b \in Component(a)
    \* The representative is the component's lowest rank, which makes the choice unique.
    /\ \A s \in Surv : \A t \in Surv : (unit[t] = unit[s]) => Rank[unit[s]] <= Rank[t]

Units == { UnitOf(s) : s \in Surv }
MembersOf(u) == { s \in Surv : UnitOf(s) = u }

----------------------------------------------------------------------------
(* The three edge classes over units. Adjacent-pair edges only; a           *)
(* topological order needs no transitive closure.                            *)

\* 1. Carrier sequence: the order a payload states between its own survivors.
CarrierEdge(u1, u2) ==
    \E o1, o2 \in Landed :
        /\ Ordered(o1) /\ Ordered(o2)
        /\ PayloadOf(o1) = PayloadOf(o2)
        /\ posn[o1] < posn[o2]
        /\ UnitOf(lineage[o1]) = u1
        /\ UnitOf(lineage[o2]) = u2

\* 2. Causality: a result follows the call it answers. No adjacency requirement -
\*    Vercel issues parallel calls whose results legitimately interleave.
CausalEdge(u1, u2) ==
    \E r \in Surv :
        /\ answers[r] # NoSurv
        /\ UnitOf(answers[r]) = u1
        /\ UnitOf(r) = u2

\* 3. Generation dataflow: what a model call received precedes what it produced.
\*    Local dataflow, deliberately not a rule about roles: a global "the final
\*    assistant message follows the last user message and every intervening
\*    tool" is false for parallel branches, subagents, retries, abandoned calls.
DataflowEdge(u1, u2) ==
    \E i, p \in Landed :
        /\ Carrier(i).gen /\ Carrier(p).gen
        /\ SpanOf(i) = SpanOf(p)
        /\ ~Carrier(i).output
        /\ Carrier(p).output
        /\ UnitOf(lineage[i]) = u1
        /\ UnitOf(lineage[p]) = u2

Edge(u1, u2) ==
    /\ u1 # u2
    /\ \/ CarrierEdge(u1, u2)
       \/ CausalEdge(u1, u2)
       \/ DataflowEdge(u1, u2)

----------------------------------------------------------------------------
(* Resolution: deterministic Kahn over units, with a total cycle fallback.  *)

MinRank(S) == CHOOSE x \in S : \A y \in S : Rank[x] <= Rank[y]

RECURSIVE Kahn(_, _)
Kahn(remaining, acc) ==
    IF remaining = {} THEN acc
    ELSE LET ready == { u \in remaining : \A v \in remaining : ~Edge(v, u) }
             \* No ready unit means the evidence contradicts itself. Release the
             \* lowest legacy index so the result is still a total order: the
             \* alternative is no answer at all, and a diagnosed arbitrary
             \* choice beats diverging.
             pick == IF ready # {} THEN MinRank(ready) ELSE MinRank(remaining)
         IN Kahn(remaining \ {pick}, Append(acc, pick))

UnitOrder == Kahn(Units, <<>>)

\* Members of one unit in legacy order. (The implementation can also use the
\* emission's own adjacency here; legacy order is the neutral choice and is what
\* the contiguity property below depends on, not the internal order.)
RECURSIVE Flatten(_, _)
Flatten(i, acc) ==
    IF i > Len(UnitOrder) THEN acc
    ELSE LET members == MembersOf(UnitOrder[i])
             RECURSIVE Emit(_, _)
             Emit(left, out) ==
                 IF left = {} THEN out
                 ELSE LET m == MinRank(left) IN Emit(left \ {m}, Append(out, m))
         IN Flatten(i + 1, Emit(members, acc))

Order == Flatten(1, <<>>)

IndexOf(x) == CHOOSE i \in 1..Len(Order) : Order[i] = x
Before(x, y) == IndexOf(x) < IndexOf(y)

----------------------------------------------------------------------------
(* Reachability, for stating "the constraints are satisfiable".             *)

RECURSIVE Reach(_, _)
Reach(frontier, seen) ==
    IF frontier = {} THEN seen
    ELSE LET next == { v \in Units : v \notin seen /\ \E u \in frontier : Edge(u, v) }
         IN Reach(next, seen \cup next)

Acyclic == \A u \in Units : u \notin Reach({u}, {})

----------------------------------------------------------------------------
(* PROPERTIES.                                                             *)

TypeOK ==
    /\ lineage \in [Obs -> Surv \cup {NoSurv}]
    /\ prof \in [Obs -> Profiles]
    /\ posn \in [Obs -> 0..2]
    /\ answers \in [Surv -> Surv \cup {NoSurv}]
    /\ unit \in [Surv -> Surv]

\* Every survivor appears exactly once: the resolver reorders, it never adds,
\* drops or duplicates. Holds unconditionally, cycles included.
Permutation ==
    /\ Len(Order) = Cardinality(Surv)
    /\ { Order[i] : i \in 1..Len(Order) } = Surv
    /\ \A i, j \in 1..Len(Order) : i # j => Order[i] # Order[j]

\* P1. Emission cohesion: one unit's members are contiguous. Unconditional -
\*     it follows from units being nodes, not from any edge, so a contradictory
\*     edge set cannot break it.
Cohesion ==
    \A u \in Units :
        \A s1, s2 \in MembersOf(u) :
            \A t \in Surv :
                (Before(s1, t) /\ Before(t, s2)) => UnitOf(t) = u

\* P2. Every enforced edge is respected, when the evidence is consistent.
EdgesRespected == \A u1, u2 \in Units : Edge(u1, u2) => Before(u1, u2)

\* P3. Causality: a matched result follows its call, across units.
Causality ==
    \A r \in Surv :
        (answers[r] # NoSurv /\ UnitOf(answers[r]) # UnitOf(r))
            => Before(answers[r], r)

\* P4. Carrier subsequence: survivors of an ordered payload keep its order,
\*     except where contraction put them in one unit, which P1 governs.
CarrierSubsequence ==
    \A o1, o2 \in Landed :
        ( /\ Ordered(o1) /\ Ordered(o2)
          /\ PayloadOf(o1) = PayloadOf(o2)
          /\ posn[o1] < posn[o2]
          /\ UnitOf(lineage[o1]) # UnitOf(lineage[o2]) )
        => Before(lineage[o1], lineage[o2])

\* P5. Dataflow: a generation's input precedes its output, where they are distinct
\*     units. A span whose input and output collapsed onto one unit states nothing
\*     to order - the same exclusion `Edge` makes, and the reason it is there.
Dataflow ==
    \A u1, u2 \in Units :
        (u1 # u2 /\ DataflowEdge(u1, u2)) => Before(u1, u2)

\* The joint claim. Note what is unconditional and what is not: the structural
\* guarantees hold always, the ordering guarantees hold whenever the evidence is
\* not self-contradictory. A cycle means two constraints disagree about the same
\* pair, and then no order can satisfy both - so the resolver owes a total order
\* and a diagnosis, not a satisfying one.
AcyclicImpliesEdges == Acyclic => EdgesRespected
AcyclicImpliesCausality == Acyclic => Causality
AcyclicImpliesCarrier == Acyclic => CarrierSubsequence
AcyclicImpliesDataflow == Acyclic => Dataflow

Correct ==
    /\ Permutation
    /\ Cohesion
    /\ Acyclic => EdgesRespected
    /\ Acyclic => Causality
    /\ Acyclic => CarrierSubsequence
    /\ Acyclic => Dataflow

----------------------------------------------------------------------------
(* Two survivors, and the scope that costs is stated in the module header: at this  *)
(* size `Cohesion` is vacuous, because no third block can land between a contracted *)
(* pair. A three-survivor model does not finish - TLC enumerates initial states      *)
(* before filtering them - and contiguity holds by construction anyway, since the     *)
(* emit loop flattens each unit's members consecutively.                              *)

(* One state per input shape: TLC enumerates the inputs and checks Correct  *)
(* on each, which is a bounded proof over all shapes rather than over the   *)
(* shapes the fixture corpus happens to contain.                           *)

(* Preconditions the pipeline guarantees, so the model does not spend its state  *)
(* space on inputs that cannot occur:                                            *)
(*   - every survivor came from at least one observation (dedup keeps a copy, it  *)
(*     does not invent blocks);                                                   *)
(*   - a tool result does not answer itself.                                      *)
WellFormedInput ==
    /\ \A s \in Surv : \E o \in Obs : lineage[o] = s
    /\ \A r \in Surv : answers[r] # r

Init ==
    /\ lineage \in [Obs -> Surv \cup {NoSurv}]
    /\ prof \in [Obs -> Profiles]
    /\ posn \in [Obs -> 0..2]
    /\ answers \in [Surv -> Surv \cup {NoSurv}]
    /\ unit \in [Surv -> Surv]
    /\ WellFormedInput
    /\ WellFormedUnits

Next == UNCHANGED vars

Spec == Init /\ [][Next]_vars

============================================================================
