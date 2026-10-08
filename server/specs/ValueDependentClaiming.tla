------------------------ MODULE ValueDependentClaiming ------------------------
(***************************************************************************)
(* Which message rule owns which carrier, when ownership depends on the     *)
(* span's values. A refinement of the kernel in `CarrierClaiming.tla`,      *)
(* where each rule's carriers were fixed: here a read takes the first of    *)
(* its spellings the span carries and owns it only if it parses, a compose  *)
(* owns exactly the members that parse, a claim owns without emitting, and   *)
(* a gate that holds owns nothing by itself. A compose member read through  *)
(* a conditional fallback (`fbs`) gives way: where an earlier rule owns its *)
(* carrier, the member is dropped and the rest of the compose kept, while   *)
(* anything is left.                                                       *)
(*                                                                         *)
(* The rules of `MessagePlan` this states, each refutable by a mutation of  *)
(* the engine:                                                             *)
(*                                                                         *)
(*   PresentModeCommits  - a read whose first present spelling does not    *)
(*                         parse owns nothing; it never retries an alias.   *)
(*   OnlyWhatWasRead     - an owner read the carrier, and it parsed.       *)
(*   AllOrNothing        - a kept reading owns everything it read except a  *)
(*                         fallback member's carrier an earlier rule owns;  *)
(*                         a reading that would take any other owned        *)
(*                         carrier is dropped whole.                        *)
(*   ClaimsBlock         - a claim owns its carrier and emits nothing, and   *)
(*                         neither a later rule nor the fallback reads it.  *)
(*   FallbackRespects... - the stage-fallback read keeps nothing a dialect   *)
(*                         rule owns.                                      *)
(*   NoComposeStarves    - in an instance the compiler accepts, a compose    *)
(*                         is dropped only where every member it read was a *)
(*                         fallback another rule owns: the refusal of an     *)
(*                         earlier reader of a member's own spelling         *)
(*                         (`StarvedReading`) is exactly what makes that      *)
(*                         hold, and `Refused` below is that refusal.         *)
(*   MatchesGreedy       - the outcome is the rank-ordered greedy function,  *)
(*                         written independently of the steps.              *)
(*   ExpectationsHold    - the manifest's hand-written answers.              *)
(*                                                                         *)
(* The instances are rendered from `instances/ValueDependentClaiming.json`  *)
(* by the Rust test `value_dependent_claiming_instances`, which runs the     *)
(* same instances over the same spans through the engine. Ownership is per   *)
(* output axis in the engine; every rule here emits on the message axis.     *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences, TLC

\* BEGIN GENERATED FROM instances/ValueDependentClaiming.json
ManifestCarriers == {"a", "b", "c"}
ManifestInstances == <<
    [id |-> "contested_alias", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "read", cands |-> <<"a", "b">>, fbs |-> {}], [id |-> "r2", prio |-> 20, kind |-> "claim", cands |-> <<"b">>, fbs |-> {}], [id |-> "r3", prio |-> 30, kind |-> "read", cands |-> <<"b", "c">>, fbs |-> {}] >>, fallback |-> <<"c">>, refused |-> FALSE, expect |-> << [values |-> ("a" :> "bad" @@ "b" :> "ok" @@ "c" :> "ok"), gates |-> {"r1", "r2", "r3"}, owners |-> ("b" :> "r2" @@ "c" :> "fallback"), kept |-> <<"r2">>, fallback |-> TRUE], [values |-> ("a" :> "ok" @@ "b" :> "ok" @@ "c" :> "absent"), gates |-> {"r1", "r2", "r3"}, owners |-> ("a" :> "r1" @@ "b" :> "r2"), kept |-> <<"r1", "r2">>, fallback |-> FALSE], [values |-> ("a" :> "absent" @@ "b" :> "ok" @@ "c" :> "ok"), gates |-> {"r3"}, owners |-> ("b" :> "r3" @@ "c" :> "fallback"), kept |-> <<"r3">>, fallback |-> TRUE] >>],
    [id |-> "compose_first", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "compose", cands |-> <<"a", "b">>, fbs |-> {}], [id |-> "r2", prio |-> 20, kind |-> "read", cands |-> <<"b">>, fbs |-> {}], [id |-> "r3", prio |-> 30, kind |-> "claim", cands |-> <<"c">>, fbs |-> {}] >>, fallback |-> <<"a">>, refused |-> FALSE, expect |-> << [values |-> ("a" :> "ok" @@ "b" :> "bad" @@ "c" :> "ok"), gates |-> {"r1", "r2", "r3"}, owners |-> ("a" :> "r1" @@ "c" :> "r3"), kept |-> <<"r1", "r3">>, fallback |-> FALSE], [values |-> ("a" :> "bad" @@ "b" :> "ok" @@ "c" :> "absent"), gates |-> {"r2"}, owners |-> ("b" :> "r2"), kept |-> <<"r2">>, fallback |-> FALSE] >>],
    [id |-> "claim_then_fallback", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "claim", cands |-> <<"a">>, fbs |-> {}] >>, fallback |-> <<"a">>, refused |-> FALSE, expect |-> << [values |-> ("a" :> "ok" @@ "b" :> "absent" @@ "c" :> "absent"), gates |-> {"r1"}, owners |-> ("a" :> "r1"), kept |-> <<"r1">>, fallback |-> FALSE] >>],
    [id |-> "starved_compose", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "read", cands |-> <<"a">>, fbs |-> {}], [id |-> "r2", prio |-> 20, kind |-> "compose", cands |-> <<"a", "b">>, fbs |-> {}] >>, fallback |-> <<>>, refused |-> TRUE, expect |-> <<>>],
    [id |-> "overlapping_composes", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "compose", cands |-> <<"a", "b">>, fbs |-> {}], [id |-> "r2", prio |-> 20, kind |-> "compose", cands |-> <<"b", "c">>, fbs |-> {}] >>, fallback |-> <<>>, refused |-> TRUE, expect |-> <<>>],
    [id |-> "fallback_gives_way", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "read", cands |-> <<"a">>, fbs |-> {}], [id |-> "r2", prio |-> 20, kind |-> "compose", cands |-> <<"b">>, fbs |-> {"a"}] >>, fallback |-> <<"c">>, refused |-> FALSE, expect |-> << [values |-> ("a" :> "ok" @@ "b" :> "ok" @@ "c" :> "absent"), gates |-> {"r1", "r2"}, owners |-> ("a" :> "r1" @@ "b" :> "r2"), kept |-> <<"r1", "r2">>, fallback |-> FALSE], [values |-> ("a" :> "ok" @@ "b" :> "ok" @@ "c" :> "ok"), gates |-> {"r2"}, owners |-> ("a" :> "r2" @@ "b" :> "r2" @@ "c" :> "fallback"), kept |-> <<"r2">>, fallback |-> TRUE], [values |-> ("a" :> "ok" @@ "b" :> "bad" @@ "c" :> "absent"), gates |-> {"r1", "r2"}, owners |-> ("a" :> "r1"), kept |-> <<"r1">>, fallback |-> FALSE] >>],
    [id |-> "fallback_first", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "compose", cands |-> <<"b">>, fbs |-> {"a"}], [id |-> "r2", prio |-> 20, kind |-> "read", cands |-> <<"a">>, fbs |-> {}] >>, fallback |-> <<>>, refused |-> FALSE, expect |-> << [values |-> ("a" :> "ok" @@ "b" :> "ok" @@ "c" :> "absent"), gates |-> {"r1", "r2"}, owners |-> ("a" :> "r1" @@ "b" :> "r1"), kept |-> <<"r1">>, fallback |-> FALSE] >>],
    [id |-> "fallback_over_a_member", rules |-> << [id |-> "r1", prio |-> 10, kind |-> "compose", cands |-> <<"b">>, fbs |-> {"a"}], [id |-> "r2", prio |-> 20, kind |-> "compose", cands |-> <<"a", "c">>, fbs |-> {}] >>, fallback |-> <<>>, refused |-> TRUE, expect |-> <<>>]
>>
\* END GENERATED FROM instances/ValueDependentClaiming.json

Values == {"absent", "bad", "ok"}
NoOwner == "none"
FallbackId == "fallback"

Range(s) == {s[k] : k \in DOMAIN s}
Inst(i) == ManifestInstances[i]
Rules(i) == Range(Inst(i).rules)
RuleIds(i) == {r.id : r \in Rules(i)}
RuleOf(i, id) == CHOOSE r \in Rules(i) : r.id = id

\* The first of a list of spellings the span carries, or NoOwner.
FirstPresent(cands, v) ==
    LET present == {k \in DOMAIN cands : v[cands[k]] # "absent"}
    IN IF present = {} THEN NoOwner
       ELSE cands[CHOOSE k \in present : \A o \in present : k <= o]

\* What a rule's reading would own if it is kept, and whether it yields at all.
\* A gate that fails yields nothing; a read commits to its first present spelling;
\* a compose yields the members that parse.
Reading(r, v, g) ==
    IF r.id \notin g THEN [yields |-> FALSE, owns |-> {}]
    ELSE IF r.kind \in {"read", "claim"}
         THEN LET c == FirstPresent(r.cands, v)
              IN IF c # NoOwner /\ v[c] = "ok"
                    THEN [yields |-> TRUE, owns |-> {c}]
                    ELSE [yields |-> FALSE, owns |-> {}]
         ELSE LET parsed == {c \in Range(r.cands) \cup r.fbs : v[c] = "ok"}
              IN [yields |-> parsed # {}, owns |-> parsed]

\* What a reading keeps against the carriers already owned: all of it where none is
\* taken; where only fallback members' carriers are, the rest, if anything is left;
\* otherwise nothing.
Keeps(r, read, owned) ==
    LET taken == read.owns \cap DOMAIN owned
    IN IF ~read.yields THEN {}
       ELSE IF taken = {} THEN read.owns
       ELSE IF taken \subseteq r.fbs THEN read.owns \ taken
       ELSE {}

\* The compile-time refusal `StarvedReading`: a rule ranked ahead of a compose of
\* several members reads, by any spelling or fallback, one of its members' own
\* spellings. A fallback member's carrier gives way, so it is not one.
Refused(i) ==
    \E later, earlier \in Rules(i) :
        /\ later.kind = "compose"
        /\ Len(later.cands) + Cardinality(later.fbs) > 1
        /\ earlier.prio < later.prio
        /\ (Range(earlier.cands) \cup earlier.fbs) \cap Range(later.cands) # {}

----------------------------------------------------------------------------
(* The specified answer, written without reference to the steps below.     *)
----------------------------------------------------------------------------

Lowest(rs) == CHOOSE r \in rs : \A o \in rs : r.prio <= o.prio

\* owned: carrier |-> owner id, over the carriers owned so far.
RECURSIVE Greedy(_, _, _, _, _, _)
Greedy(i, v, g, owned, kept, remaining) ==
    IF remaining = {} THEN [owned |-> owned, kept |-> kept]
    ELSE LET r == Lowest(remaining)
             keeps == Keeps(r, Reading(r, v, g), owned)
             takes == keeps # {}
             after == IF takes
                        THEN [c \in DOMAIN owned \cup keeps |->
                                IF c \in keeps THEN r.id ELSE owned[c]]
                        ELSE owned
         IN Greedy(i, v, g, after, IF takes THEN Append(kept, r.id) ELSE kept, remaining \ {r})

DialectAnswer(i, v, g) == Greedy(i, v, g, <<>>, <<>>, Rules(i))

FallbackCarrier(i, v) ==
    LET c == FirstPresent(Inst(i).fallback, v)
    IN IF c # NoOwner /\ v[c] = "ok" THEN c ELSE NoOwner

GreedyOwners(i, v, g) ==
    LET d == DialectAnswer(i, v, g).owned
        c == FallbackCarrier(i, v)
    IN IF c # NoOwner /\ c \notin DOMAIN d
          THEN [x \in DOMAIN d \cup {c} |-> IF x = c THEN FallbackId ELSE d[x]]
          ELSE d

GreedyFallback(i, v, g) ==
    LET c == FallbackCarrier(i, v)
    IN c # NoOwner /\ c \notin DOMAIN DialectAnswer(i, v, g).owned

----------------------------------------------------------------------------
(* The manifest, held to the definitions above.                            *)
----------------------------------------------------------------------------

\* The manifest's `refused` flag is the model's refusal, and the Rust test holds the
\* compiler to the same flag.
ASSUME \A i \in DOMAIN ManifestInstances : Inst(i).refused = Refused(i)

\* Every hand-written answer is the definition's answer.
ASSUME \A i \in DOMAIN ManifestInstances :
    \A k \in DOMAIN Inst(i).expect :
        LET e == Inst(i).expect[k]
        IN /\ GreedyOwners(i, e.values, e.gates) = e.owners
           /\ DialectAnswer(i, e.values, e.gates).kept = e.kept
           /\ GreedyFallback(i, e.values, e.gates) = e.fallback

----------------------------------------------------------------------------
VARIABLES inst, val, gate, pending, owner, kept, fbKept, phase

vars == <<inst, val, gate, pending, owner, kept, fbKept, phase>>

Init ==
    /\ inst \in DOMAIN ManifestInstances
    /\ val \in [ManifestCarriers -> Values]
    /\ gate \in SUBSET RuleIds(inst)
    /\ pending = Rules(inst)
    /\ owner = <<>>
    /\ kept = <<>>
    /\ fbKept = FALSE
    /\ phase = "dialect"

\* The lowest-priority rule not yet seen: it keeps what `Keeps` leaves it - all of
\* its reading, or all of it but the fallback members another rule owns - and then
\* owns that.
Evaluate ==
    /\ phase = "dialect"
    /\ pending # {}
    /\ LET r == Lowest(pending)
           keeps == Keeps(r, Reading(r, val, gate), owner)
           takes == keeps # {}
       IN /\ pending' = pending \ {r}
          /\ owner' = IF takes
                        THEN [c \in DOMAIN owner \cup keeps |->
                                IF c \in keeps THEN r.id ELSE owner[c]]
                        ELSE owner
          /\ kept' = IF takes THEN Append(kept, r.id) ELSE kept
    /\ UNCHANGED <<inst, val, gate, fbKept, phase>>

\* The stage-fallback read, given what the dialect rules own.
FallBack ==
    /\ phase = "dialect"
    /\ pending = {}
    /\ LET c == FirstPresent(Inst(inst).fallback, val)
           takes == c # NoOwner /\ val[c] = "ok" /\ c \notin DOMAIN owner
       IN /\ owner' = IF takes THEN [x \in DOMAIN owner \cup {c} |->
                                       IF x = c THEN FallbackId ELSE owner[x]]
                                ELSE owner
          /\ fbKept' = takes
    /\ phase' = "done"
    /\ UNCHANGED <<inst, val, gate, pending, kept>>

Done == phase = "done" /\ UNCHANGED vars

Next == Evaluate \/ FallBack \/ Done

Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Invariants                                                              *)
----------------------------------------------------------------------------

KeptRules == {RuleOf(inst, kept[k]) : k \in DOMAIN kept}

TypeOK ==
    /\ inst \in DOMAIN ManifestInstances
    /\ val \in [ManifestCarriers -> Values]
    /\ gate \subseteq RuleIds(inst)
    /\ pending \subseteq Rules(inst)
    /\ DOMAIN owner \subseteq ManifestCarriers
    /\ \A c \in DOMAIN owner : owner[c] \in RuleIds(inst) \cup {FallbackId}
    /\ fbKept \in BOOLEAN
    /\ phase \in {"dialect", "done"}

\* The outcome is the specified one.
MatchesGreedy ==
    phase = "done" =>
        /\ owner = GreedyOwners(inst, val, gate)
        /\ kept = DialectAnswer(inst, val, gate).kept
        /\ fbKept = GreedyFallback(inst, val, gate)

\* An owner read the carrier, its gate held, and the carrier parsed.
OnlyWhatWasRead ==
    \A c \in DOMAIN owner :
        /\ val[c] = "ok"
        /\ IF owner[c] = FallbackId
              THEN c = FirstPresent(Inst(inst).fallback, val)
              ELSE /\ owner[c] \in gate
                   /\ c \in Reading(RuleOf(inst, owner[c]), val, gate).owns

\* A read whose first present spelling does not parse is never kept, and a kept
\* read owns that first spelling - it never retries an alias.
PresentModeCommits ==
    \A r \in Rules(inst) :
        r.kind \in {"read", "claim"} =>
            LET c == FirstPresent(r.cands, val)
            IN /\ (c = NoOwner \/ val[c] # "ok") => r \notin KeptRules
               /\ r \in KeptRules => owner[c] = r.id

\* A kept reading owns everything it read, except a fallback member's carrier,
\* which another rule owns; and no reading keeps part of what it read where a
\* carrier other than such a fallback was taken.
AllOrNothing ==
    \A r \in KeptRules : \A c \in Reading(r, val, gate).owns :
        \/ owner[c] = r.id
        \/ c \in r.fbs /\ c \in DOMAIN owner

\* A claim's carrier stays the claim's, whatever comes after it.
ClaimsBlock ==
    \A r \in KeptRules :
        r.kind = "claim" => \A c \in Reading(r, val, gate).owns : owner[c] = r.id

\* The fallback keeps nothing a dialect rule owns.
FallbackRespectsOwnership ==
    (phase = "done" /\ fbKept) =>
        LET c == FirstPresent(Inst(inst).fallback, val)
        IN /\ owner[c] = FallbackId
           /\ \A r \in KeptRules : c \notin Reading(r, val, gate).owns

\* Where the compiler accepts the instance, a compose whose members parse is kept,
\* unless every member it read was a fallback another rule owns.
NoComposeStarves ==
    (phase = "done" /\ ~Refused(inst)) =>
        \A r \in Rules(inst) :
            (r.kind = "compose" /\ Reading(r, val, gate).yields /\ r \notin KeptRules) =>
                \A c \in Reading(r, val, gate).owns : c \in r.fbs /\ owner[c] # r.id

\* A fallback member is never also a member's own spelling of the same compose: a
\* direct use is the dialect's own and does not give way.
ASSUME \A i \in DOMAIN ManifestInstances :
    \A r \in Rules(i) : r.fbs \cap Range(r.cands) = {} /\ (r.fbs # {} => r.kind = "compose")

\* And the refusal is not vacuous: every refused instance has a span on which, were it
\* accepted, a compose whose members parse would be dropped.
ASSUME \A i \in DOMAIN ManifestInstances :
    Refused(i) =>
        \E v \in [ManifestCarriers -> Values], g \in SUBSET RuleIds(i) :
            \E r \in Rules(i) :
                /\ r.kind = "compose"
                /\ Reading(r, v, g).yields
                /\ ~\E k \in DOMAIN DialectAnswer(i, v, g).kept :
                       DialectAnswer(i, v, g).kept[k] = r.id

============================================================================
