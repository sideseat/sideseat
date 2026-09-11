------------------------- MODULE OrderedResolution -------------------------
(***************************************************************************)
(* **Which of several matching rules answers**, and whether refusing an     *)
(* unreachable one at compile time is sound.                               *)
(*                                                                         *)
(* Three ordered first-match sweeps share this shape - framework detection, *)
(* observation type and span category - and review cycles 17 and 18 changed *)
(* two things about it that are worth stating as theorems rather than as    *)
(* prose:                                                                  *)
(*                                                                         *)
(*   1. `supersedes` **orders**, ahead of `legacy_rank`. It used to waive    *)
(*      only an overlap report while rank decided the winner regardless, so *)
(*      the field documented an ordering it took no part in and every edge   *)
(*      could have been deleted without changing an answer.                *)
(*                                                                         *)
(*   2. A rule an earlier rule always satisfies is **refused at compile     *)
(*      time**, because its answer can never be reached. That refusal is an *)
(*      over-approximation implemented over syntax, and its correctness is   *)
(*      the property below: it must never refuse a rule that could win.      *)
(*                                                                         *)
(* WHAT THIS IS NOT A MODEL OF. It is not a refinement of `DetectPlan` or   *)
(* `ClassifyPlan`. Four differences, each real:                            *)
(*                                                                         *)
(*   - A rule's conditions are abstracted to **the set of spans it matches**.*)
(*     The engine decides that from attributes, prefixes and phrase         *)
(*     searches; here it is given. That is deliberate, and it is what makes  *)
(*     `RefusalIsSound` a check of the *consequence* the syntactic          *)
(*     implication relation exists to establish - "every span matching the   *)
(*     later rule matches the earlier one" - rather than a restatement of    *)
(*     the relation itself. Whether `Atom::implies` establishes that         *)
(*     containment is a question for `detect_rules.rs` and its tests; given  *)
(*     that it does, this says what follows.                               *)
(*                                                                         *)
(*   - Only one sweep is modelled. The engine runs three, and a rank means  *)
(*     nothing across them.                                                *)
(*                                                                         *)
(*   - `alternatives` are absent. They are compiled into ordinary rules     *)
(*     sharing a label, so they are already inside "a rule" here.          *)
(*                                                                         *)
(*   - The declaration fallback (a producer naming itself through an SDK    *)
(*     slug) is absent: it runs only when no rule answered, and this spec    *)
(*     is about which rule does.                                           *)
(*                                                                         *)
(* This is a **state predicate** model: resolution is a function of the      *)
(* ruleset and the span, so there are no steps to interleave. The variable   *)
(* is the span, and TLC enumerates every one of them.                       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    \* The spans a ruleset may be asked about. Abstract identities.
    Spans,
    \* The rules of one ordered sweep.
    Rules,
    \* Rank per rule: lower is consulted first. Distinct, as every sweep refuses
    \* a shared rank - which is what makes this order total.
    Rank,
    \* The spans each rule matches. The abstraction described above.
    Matches,
    \* The declared `supersedes` edges: <<a, b>> means a beats b where both match.
    Supersedes

ASSUME Spans \subseteq Nat
ASSUME Rules \subseteq Nat
ASSUME Rank \in [Rules -> Nat]
ASSUME Matches \in [Rules -> SUBSET Spans]
ASSUME Supersedes \subseteq (Rules \X Rules)
\* Distinct ranks: refused by every sweep's compiler, so a fact about the
\* modelled scope rather than a convenience.
ASSUME \A r1, r2 \in Rules : r1 # r2 => Rank[r1] # Rank[r2]

VARIABLES span

vars == <<span>>

NoRule == 0

----------------------------------------------------------------------------
(* The `supersedes` relation, transitively - because a rule that beats a    *)
(* rule which beats a third beats the third too. Compilation refuses a      *)
(* cycle, which `AcyclicSupersedes` states as a fact about the instance.    *)
(*                                                                         *)
(* Computed as a bounded fixpoint over `Cardinality(Rules)` rounds: any     *)
(* path in a finite acyclic relation is at most that long, so the round     *)
(* count is a bound and not a guess.                                       *)
----------------------------------------------------------------------------
RECURSIVE Closure(_, _)
Closure(edges, rounds) ==
    IF rounds = 0 THEN edges
    ELSE LET grown == edges \cup
                      {<<a, c>> \in Rules \X Rules :
                          \E b \in Rules : <<a, b>> \in edges /\ <<b, c>> \in edges}
         IN IF grown = edges THEN edges ELSE Closure(grown, rounds - 1)

Beats == Closure(Supersedes, Cardinality(Rules))

BeatsRule(a, b) == <<a, b>> \in Beats

----------------------------------------------------------------------------
(* Resolution, written as the engine's own two-part answer.                 *)
----------------------------------------------------------------------------

Matching(s) == {r \in Rules : s \in Matches[r]}

\* Rules some *matching* rule beats. A rule beaten by a rule that does not
\* match this span is not beaten here - which is the whole reason `resolve`
\* collects the matching set before deciding.
BeatenIn(s) == {r \in Matching(s) : \E o \in Matching(s) : o # r /\ BeatsRule(o, r)}

\* The winner: the lowest-ranked matching rule no other matching rule beats.
\*
\* Deliberately *not* "the first matching rule that beats the rank-winner",
\* which in rank order is satisfied by the rank-winner itself and orders
\* nothing. That was the first implementation, and its test caught it.
Unbeaten(s) == Matching(s) \ BeatenIn(s)

Winner(s) ==
    IF Unbeaten(s) = {} THEN NoRule
    ELSE CHOOSE r \in Unbeaten(s) : \A o \in Unbeaten(s) : Rank[r] <= Rank[o]

\* What rank alone would have answered - the behaviour before `supersedes`
\* ordered anything, kept so the difference is a checkable property rather
\* than a claim in a commit message.
RankWinner(s) ==
    IF Matching(s) = {} THEN NoRule
    ELSE CHOOSE r \in Matching(s) : \A o \in Matching(s) : Rank[r] <= Rank[o]

----------------------------------------------------------------------------
Init == span \in Spans
Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Invariants                                                              *)
----------------------------------------------------------------------------

TypeOK == span \in Spans

\* A cycle is refused at compile time, so no rule beats itself. Stated as an
\* invariant rather than assumed: an instance that violated it would make
\* `Winner` ill-defined, and TLC should say so rather than diverge.
AcyclicSupersedes == \A r \in Rules : ~BeatsRule(r, r)

\* Exactly one rule answers where any matches. The "at most one" half is
\* `Winner` being a function; the content is "at least one" - a span some rule
\* matches is never left unanswered, which a naive "drop everything beaten"
\* would break if the beats relation had a cycle among matching rules.
UniqueWinner == Matching(span) # {} => Winner(span) \in Matching(span)

\* Nothing that matched beats the winner. This is what `supersedes` ordering
\* *means*, and it is false of rank alone.
WinnerIsUnbeaten ==
    \A o \in Matching(span) : o # Winner(span) => ~BeatsRule(o, Winner(span))

\* The winner is the lowest-ranked among those nothing beats - so rank still
\* decides between rules whose relative order nothing declared, which is the
\* half `legacy_rank` keeps doing.
RankDecidesAmongUnbeaten ==
    \A o \in Unbeaten(span) : Rank[Winner(span)] <= Rank[o]

\* **`supersedes` changes the answer.** Where a matching rule beats the
\* rank-winner, the rank-winner does not answer. Recorded as an invariant
\* because the field previously did *not* do this: every declared edge could
\* have been deleted with no answer changing, and nothing said so.
SupersedesOverridesRank ==
    (Matching(span) # {} /\ RankWinner(span) \in BeatenIn(span))
        => Winner(span) # RankWinner(span)

\* **The soundness of the compile-time refusal.** A rule an earlier rule
\* always satisfies, and which does not beat that earlier rule, can never
\* win - for any span. So refusing it at compile time removes nothing an
\* asset author could have relied on.
\*
\* This is the property the syntactic implication relation in
\* `detect_rules.rs` exists to establish, and the reason it is deliberately
\* sound-but-incomplete: a *false* containment claim here would refuse a rule
\* that can win, which is a build broken for a reason nobody can act on.
RefusalIsSound ==
    \A earlier, later \in Rules :
        (earlier # later
         /\ Rank[earlier] < Rank[later]
         /\ Matches[later] \subseteq Matches[earlier]
         /\ ~BeatsRule(later, earlier))
            => Winner(span) # later

\* **And the exemption is necessary.** A rule that *does* beat its shadower
\* wins where it matches - so refusing it would have removed a reachable
\* answer, which is exactly what the first version of the refusal did until
\* the existing `supersedes` tests caught it.
SupersedesRescuesAShadowedRule ==
    \A earlier, later \in Rules :
        (earlier # later
         /\ Rank[earlier] < Rank[later]
         /\ Matches[later] \subseteq Matches[earlier]
         /\ BeatsRule(later, earlier)
         /\ span \in Matches[later])
            => Winner(span) = later

----------------------------------------------------------------------------
(* The model instance.                                                     *)
(*                                                                         *)
(* Here rather than in the `.cfg`, because a configuration file cannot hold *)
(* a TLA+ function expression. Chosen to contain the cases the mechanism    *)
(* exists for:                                                             *)
(*                                                                         *)
(*   rule 1 is the broad one - it matches every span, as a convention       *)
(*   namespace rule does;                                                   *)
(*                                                                         *)
(*   rule 2 matches a subset of rule 1 and beats it, which is the shape     *)
(*   `supersedes` exists for: a narrow rule that must win although it is    *)
(*   ranked later, without moving its own weaker signals up too. It is also *)
(*   the shape the shadowing refusal must **not** refuse;                   *)
(*                                                                         *)
(*   rule 3 matches a subset of rule 1 and does *not* beat it, so it can    *)
(*   never win - the shape the refusal exists to catch;                     *)
(*                                                                         *)
(*   rule 4 matches spans the others do not, so it wins alone and shows the *)
(*   invariants are not vacuous.                                            *)
(*                                                                         *)
(* Spans 1..4: span 4 is matched only by rule 4, span 1 by everything.      *)
----------------------------------------------------------------------------
ModelRank == [r \in Rules |-> r]

ModelMatches ==
    [r \in Rules |->
        IF r = 1 THEN {1, 2, 3}
        ELSE IF r = 2 THEN {1, 2}
        ELSE IF r = 3 THEN {1, 3}
        ELSE {4}]

\* Rule 2 beats rule 1; rule 3 declares nothing, so it is the unreachable one.
ModelSupersedes == {<<2, 1>>}

============================================================================
