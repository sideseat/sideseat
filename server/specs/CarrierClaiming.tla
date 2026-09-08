-------------------------- MODULE CarrierClaiming --------------------------
(***************************************************************************)
(* An abstract **fixed-ownership greedy kernel**: which rule gets to read a *)
(* span's attribute, when each rule's ownership set is known in advance.    *)
(*                                                                         *)
(* WHAT THIS IS NOT A MODEL OF.  It is deliberately not a refinement model *)
(* of `MessagePlan`, and claiming otherwise was this spec's first mistake. *)
(* Three differences, each real:                                           *)
(*                                                                         *)
(*   1. It assumes distinct ranks.  Message compilation does *not* require *)
(*      them; equal ranks are broken by rule id, which is a defect recorded *)
(*      elsewhere, not a property this spec may assume away.               *)
(*                                                                         *)
(*   2. Ownership here is fixed per rule.  In the engine it is *value      *)
(*      dependent*: `from_any_of` selects whichever spelling the span      *)
(*      carries, a sweep discovers keys at read time, and a compose emits   *)
(*      when any member filled - so the ownership set is a property of the  *)
(*      emission, not of the rule.                                        *)
(*                                                                         *)
(*   3. Fallback here runs only when nothing emitted or claimed.  The      *)
(*      engine may also run it after dialect output, when a generation      *)
(*      span's answer is unaccounted for.                                  *)
(*                                                                         *)
(* What the kernel is still worth checking: the interaction between rank    *)
(* order and all-or-nothing ownership, which is where the starvation        *)
(* counterexample below comes from, and which holds in the engine for the   *)
(* same reason it holds here.                                              *)
(*                                                                         *)
(* Several dialects describe the same attribute - `output.value` is read by *)
(* four of them - and every message the server shows comes from exactly    *)
(* one rule's reading of it. `server/docs/framework-rules-engine.md` and   *)
(* the module comment on `message_rules.rs` state the rules in prose. This *)
(* spec states them as invariants so TLC can try to break them across       *)
(* every interleaving of evaluation order, which is the dimension a test    *)
(* over a fixed ruleset cannot vary.                                       *)
(*                                                                         *)
(* What is modelled, and why each is here rather than trusted:             *)
(*                                                                         *)
(*   OneReaderPerCarrier  - no attribute is read by two rules. This is the *)
(*                          property that keeps a message from appearing    *)
(*                          twice, and it is the reason `owns` is a *set*  *)
(*                          of physical carriers rather than the emitted    *)
(*                          tag: a rule composing three attributes into    *)
(*                          one observation must own all three.            *)
(*                                                                         *)
(*   ClaimTakesCarrier    - a rule may own a carrier without emitting      *)
(*                          anything. That is how a payload which is       *)
(*                          framework internals rather than a conversation  *)
(*                          is taken off the table, and it must suppress    *)
(*                          the generic fallback exactly as an emission     *)
(*                          does.                                          *)
(*                                                                         *)
(*   FallbackIsLastResort - the generic reading runs if and only if no      *)
(*                          dialect rule emitted or claimed. Both halves    *)
(*                          matter: without "only if" a span gets its       *)
(*                          payload twice, without "if" a span whose shape  *)
(*                          no dialect describes returns nothing.           *)
(*                                                                         *)
(*   GreedyInRankOrder    - within the kernel, the outcome is the           *)
(*                          rank-ordered greedy one. Note                    *)
(*                          what this is *not*: "the lowest-ranked rule that *)
(*                          reads a carrier gets it". TLC refuted that       *)
(*                          within seconds of the spec being written, and    *)
(*                          the counterexample is a real property of the     *)
(*                          engine rather than a modelling slip - see        *)
(*                          ComposedReadingsCanBeStarved below.              *)
(*                                                                         *)
(*   MoreCarriersOnlyAdd  - monotonicity. Making a further attribute        *)
(*                          visible never leaves a carrier unowned that had  *)
(*                          an owner without it. It may change *which* rule  *)
(*                          - a composed reading that only became eligible   *)
(*                          with the extra attribute can take a carrier from  *)
(*                          nobody - which is why the invariant is about     *)
(*                          ownership existing rather than about identity.    *)
(*                          `reading_more_carriers_only_adds_messages` in   *)
(*                          the Rust suite checks one corpus instance.       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    \* The physical attributes a span may carry.
    Carriers,
    \* The dialect rules, each with a rank and the carriers it can read.
    Rules,
    \* Rank per rule: lower wins. Distinct, as the compiler requires.
    Rank,
    \* Which carriers a rule reads, if it is the one that gets them.
    Reads,
    \* Which of those rules only *claim*: they own the carrier and emit nothing.
    ClaimOnly

ASSUME Carriers \subseteq Nat
ASSUME Rules \subseteq Nat
ASSUME Rank \in [Rules -> Nat]
ASSUME Reads \in [Rules -> SUBSET Carriers]
ASSUME ClaimOnly \subseteq Rules
\* Distinct ranks. **An assumption of the kernel, not a fact about the engine**:
\* classification refuses a shared rank and detection does too, but *message*
\* compilation does not - it breaks a tie by rule id, so renaming a rule can
\* change behaviour. That is a defect recorded in the Rust tree rather than a
\* property this spec may assume; here it is stated as the assumption it is.
ASSUME \A r1, r2 \in Rules : r1 # r2 => Rank[r1] # Rank[r2]

VARIABLES
    \* The attributes this span actually carries. Chosen once, then fixed.
    present,
    \* Rules not yet evaluated. Shrinks in an arbitrary order.
    pending,
    \* carrier |-> the rule that owns it, or 0 for unowned.
    ownedBy,
    \* Rules that produced a message.
    emitted,
    \* Rules that owned without emitting.
    claimed,
    \* TRUE once the generic fallback stage has run.
    fellBack

vars == <<present, pending, ownedBy, emitted, claimed, fellBack>>

NoRule == 0

----------------------------------------------------------------------------
(* The rules a span's contents make eligible, and who should win each      *)
(* carrier. These are the *specification* of the answer, written           *)
(* independently of the steps that compute it - which is what lets          *)
(* RankDecides be a real check rather than a restatement.                   *)
----------------------------------------------------------------------------

\* A rule is eligible when the span carries every attribute it reads. A rule
\* reading nothing present contributes nothing.
Eligible == {r \in Rules : Reads[r] # {} /\ Reads[r] \subseteq present}

\* The eligible rules that read this carrier at all.
Contenders(c) == {r \in Eligible : c \in Reads[r]}

\* The rank-ordered greedy outcome, computed independently of the steps below -
\* which is what makes GreedyInRankOrder a check rather than a restatement.
\*
\* Greedy, not "lowest contender wins": a rule takes a carrier only if it can
\* take *every* carrier it reads, so a rank-1 rule reading one attribute can
\* leave a rank-4 rule that composes that attribute with another unable to take
\* either. The composed rule then reads nothing, and its second attribute ends
\* up owned by nobody even though a rule reads it.
RECURSIVE GreedyFrom(_, _)
GreedyFrom(owned, remaining) ==
    IF remaining = {} THEN owned
    ELSE LET next == CHOOSE r \in remaining :
                        \A o \in remaining : Rank[r] <= Rank[o]
             wants == Reads[next]
             takes == wants # {}
                      /\ wants \subseteq present
                      /\ \A c \in wants : owned[c] = NoRule
             after == IF takes
                        THEN [c \in Carriers |->
                                IF c \in wants THEN next ELSE owned[c]]
                        ELSE owned
         IN GreedyFrom(after, remaining \ {next})

Greedy == GreedyFrom([c \in Carriers |-> NoRule], Rules)

----------------------------------------------------------------------------
Init ==
    /\ present \in SUBSET Carriers
    /\ pending = Rules
    /\ ownedBy = [c \in Carriers |-> NoRule]
    /\ emitted = {}
    /\ claimed = {}
    /\ fellBack = FALSE

\* Evaluate the lowest-ranked rule not yet seen. **In rank order**, because that
\* is what the engine does: the compiler sorts by rank and the runner walks the
\* sorted list, and a shared rank is refused precisely so this order is total.
\*
\* Modelling arbitrary order instead was the spec's first form, and TLC refuted
\* it immediately - correctly, since a greedy claim in arbitrary order genuinely
\* gives different answers. That refutation is what made the ordering guarantee
\* worth writing down rather than assuming.
Evaluate(r) ==
    /\ r \in pending
    /\ \A other \in pending : Rank[r] <= Rank[other]
    /\ ~fellBack
    /\ pending' = pending \ {r}
    /\ LET wants == Reads[r]
           free == \A c \in wants : ownedBy[c] = NoRule
           takes == wants # {} /\ wants \subseteq present /\ free
       IN /\ ownedBy' = IF takes
                          THEN [c \in Carriers |->
                                  IF c \in wants THEN r ELSE ownedBy[c]]
                          ELSE ownedBy
          /\ emitted' = IF takes /\ r \notin ClaimOnly
                          THEN emitted \cup {r} ELSE emitted
          /\ claimed' = IF takes /\ r \in ClaimOnly
                          THEN claimed \cup {r} ELSE claimed
    /\ UNCHANGED <<present, fellBack>>

\* The generic reading, once every dialect rule has been evaluated and none
\* of them owned anything.
FallBack ==
    /\ pending = {}
    /\ ~fellBack
    /\ emitted = {} /\ claimed = {}
    /\ fellBack' = TRUE
    /\ UNCHANGED <<present, pending, ownedBy, emitted, claimed>>

\* Evaluation is over: every rule seen, and the fallback either ran or was
\* not needed. Modelled explicitly so TLC has a terminal state rather than a
\* deadlock to report.
Done ==
    /\ pending = {}
    /\ (fellBack \/ emitted # {} \/ claimed # {})
    /\ UNCHANGED vars

Next == (\E r \in Rules : Evaluate(r)) \/ FallBack \/ Done

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

----------------------------------------------------------------------------
(* Invariants                                                              *)
----------------------------------------------------------------------------

TypeOK ==
    /\ present \subseteq Carriers
    /\ pending \subseteq Rules
    /\ ownedBy \in [Carriers -> Rules \cup {NoRule}]
    /\ emitted \subseteq Rules
    /\ claimed \subseteq Rules
    /\ fellBack \in BOOLEAN

\* No carrier has two owners. Trivially true of a function, so the content is
\* the second conjunct: an owner owns *every* carrier it reads, so a rule can
\* never have half of a composed reading while another rule holds the rest.
OneReaderPerCarrier ==
    \A r \in emitted \cup claimed :
        \A c \in Reads[r] : ownedBy[c] = r

\* An owned carrier is never available to anything else, whether its owner
\* emitted or merely claimed.
ClaimTakesCarrier ==
    \A c \in Carriers :
        ownedBy[c] # NoRule => ownedBy[c] \in (emitted \cup claimed)

\* The generic reading ran exactly when no dialect rule took anything. The
\* "only if" half is what keeps a span from reporting its payload twice.
FallbackIsLastResort ==
    fellBack => (emitted = {} /\ claimed = {})

\* Once evaluation is complete, the outcome is exactly the rank-ordered greedy
\* one - computed above without reference to these steps.
GreedyInRankOrder ==
    pending = {} => ownedBy = Greedy

\* **A composed reading can be starved, and that is the engine's behaviour.**
\*
\* Stated as an invariant so it is recorded rather than discovered: a carrier can
\* end up owned by nobody even though an eligible rule reads it, when a
\* lower-ranked rule took one of that rule's other carriers first. TLC refuting
\* "lowest contender wins" is what surfaced it.
\*
\* Whether it is *desirable* is a question for the assets, not for this spec: a
\* composed rule that must not be starved has to be ranked above the rules that
\* would take its parts, and nothing today checks that. This invariant is the
\* honest form - it says the situation is reachable.
ComposedReadingsCanBeStarved ==
    \A c \in Carriers :
        (pending = {} /\ ownedBy[c] = NoRule /\ Contenders(c) # {})
            => \E r \in Contenders(c) :
                  \E other \in Reads[r] :
                     ownedBy[other] # NoRule /\ ownedBy[other] # r

\* Nothing is owned by a rule that was not eligible: a rule whose reading is
\* not fully present must take nothing at all.
OnlyEligibleOwn ==
    \A c \in Carriers :
        ownedBy[c] # NoRule => ownedBy[c] \in Eligible

----------------------------------------------------------------------------
(* Monotonicity, as a state predicate over the *specified* answer rather   *)
(* than over a run: adding a carrier to a span cannot take an owner away    *)
(* from a carrier that was already there.                                  *)
(*                                                                         *)
(* Stated over `Winner` because that is a function of `present` alone, so   *)
(* the two spans can be compared without running the model twice - which is *)
(* what a temporal property would need and TLC cannot express here.         *)
----------------------------------------------------------------------------
RECURSIVE GreedyWith(_, _, _)
GreedyWith(p, owned, remaining) ==
    IF remaining = {} THEN owned
    ELSE LET next == CHOOSE r \in remaining :
                        \A o \in remaining : Rank[r] <= Rank[o]
             wants == Reads[next]
             takes == wants # {}
                      /\ wants \subseteq p
                      /\ \A c \in wants : owned[c] = NoRule
             after == IF takes
                        THEN [c \in Carriers |->
                                IF c \in wants THEN next ELSE owned[c]]
                        ELSE owned
         IN GreedyWith(p, after, remaining \ {next})

GreedyFor(p) == GreedyWith(p, [c \in Carriers |-> NoRule], Rules)

MoreCarriersOnlyAdd ==
    \A extra \in Carriers :
        \A c \in present :
            \* A carrier already present keeps *an* owner if it had one. Which
            \* rule may change, because a composed reading can only become
            \* eligible once the extra attribute is there - but the carrier
            \* never becomes unowned, which is the message disappearing.
            GreedyFor(present)[c] # NoRule => GreedyFor(present \cup {extra})[c] # NoRule

----------------------------------------------------------------------------
(* The model instance.                                                     *)
(*                                                                         *)
(* Here rather than in the `.cfg`, because a configuration file cannot hold *)
(* a TLA+ function expression - only simple values. Substituted in by the   *)
(* cfg's `Rank <- ModelRank` form, so the spec above stays parameterised    *)
(* and this is the one place the instance is described.                     *)
(*                                                                         *)
(* Chosen to contain the cases that motivate the mechanism:                 *)
(*                                                                         *)
(*   rules 1 and 2 contend for carrier 1, which is the real situation -     *)
(*   four dialects describe `output.value`;                                 *)
(*                                                                         *)
(*   rule 3 *claims* carrier 2 and emits nothing, which is how a payload    *)
(*   that is framework internals is taken off the table;                    *)
(*                                                                         *)
(*   rule 4 composes carriers 1 and 3, so it can only take both or neither  *)
(*   - the property that makes `owns` a set.                               *)
----------------------------------------------------------------------------
ModelRank == [r \in Rules |-> r]

ModelReads ==
    [r \in Rules |->
        IF r = 1 THEN {1}
        ELSE IF r = 2 THEN {1}
        ELSE IF r = 3 THEN {2}
        ELSE {1, 3}]

ModelClaimOnly == {3}

============================================================================
