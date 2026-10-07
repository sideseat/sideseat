------------------------ MODULE ThreeValuedPredicate ------------------------
(***************************************************************************)
(* **The logic of every `where`**: strong Kleene over true, false and       *)
(* unknown, where a condition holds only when it is true.                  *)
(*                                                                         *)
(* An atom that cannot ask its question - a value test on an absent        *)
(* attribute, a path that selects nothing - answers unknown, so negating   *)
(* it does not make it hold: absence is asked about by asking about        *)
(* presence (`exists`), which is total. This module states the operators  *)
(* and checks, over every assignment, the laws an author relies on when    *)
(* rewriting a condition (commutativity, associativity, distributivity,   *)
(* absorption, De Morgan, double negation), the one law that fails         *)
(* (excluded middle, exactly on unknown), and that an n-ary group answers *)
(* the same whether it is folded eagerly or evaluated with the early exits *)
(* `Expr::eval` takes.                                                     *)
(*                                                                         *)
(* The operator tables are written by hand in                              *)
(* `server/specs/instances/ThreeValuedPredicate.json` and generated into   *)
(* the section below; `ManifestTablesHold` checks this model's operators   *)
(* against them, and the Rust test `three_valued_predicate_instances`     *)
(* checks the production `Expr::eval` against the same tables, the same   *)
(* laws over every expression tree within the manifest's bounds, and the  *)
(* manifest's atom cases through the production lowering and evaluators.  *)
(***************************************************************************)
EXTENDS Integers, Sequences, TLC

\* BEGIN GENERATED FROM instances/ThreeValuedPredicate.json
ManifestDepth == 2
ManifestLeaves == 3
TableNot == ("T" :> "F" @@ "F" :> "T" @@ "U" :> "U")
TableAll == ("TT" :> "T" @@ "TF" :> "F" @@ "TU" :> "U" @@ "FT" :> "F" @@ "FF" :> "F" @@ "FU" :> "F" @@ "UT" :> "U" @@ "UF" :> "F" @@ "UU" :> "U")
TableAny == ("TT" :> "T" @@ "TF" :> "T" @@ "TU" :> "T" @@ "FT" :> "T" @@ "FF" :> "F" @@ "FU" :> "U" @@ "UT" :> "T" @@ "UF" :> "U" @@ "UU" :> "U")
\* END GENERATED FROM instances/ThreeValuedPredicate.json

Values == {"T", "F", "U"}

Holds(v) == v = "T"

Not(a) == IF a = "T" THEN "F" ELSE IF a = "F" THEN "T" ELSE "U"

And(a, b) == IF a = "F" \/ b = "F" THEN "F" ELSE IF a = "T" /\ b = "T" THEN "T" ELSE "U"

Or(a, b) == IF a = "T" \/ b = "T" THEN "T" ELSE IF a = "F" /\ b = "F" THEN "F" ELSE "U"

\* A group of any arity, folded over every child.
RECURSIVE AllEager(_), AnyEager(_)
AllEager(s) == IF s = <<>> THEN "T" ELSE And(Head(s), AllEager(Tail(s)))
AnyEager(s) == IF s = <<>> THEN "F" ELSE Or(Head(s), AnyEager(Tail(s)))

\* The same group evaluated the way `Expr::eval` does: a conjunction stops at
\* the first false, a disjunction at the first true, and otherwise remembers
\* whether any child was unknown.
RECURSIVE AllShort(_, _), AnyShort(_, _)
AllShort(s, sofar) ==
    IF s = <<>> THEN sofar
    ELSE IF Head(s) = "F" THEN "F"
    ELSE AllShort(Tail(s), IF Head(s) = "U" THEN "U" ELSE sofar)
AnyShort(s, sofar) ==
    IF s = <<>> THEN sofar
    ELSE IF Head(s) = "T" THEN "T"
    ELSE AnyShort(Tail(s), IF Head(s) = "U" THEN "U" ELSE sofar)

Groups == UNION {[1..n -> Values] : n \in 2..ManifestLeaves}

VARIABLES a, b, c

vars == <<a, b, c>>

Init == a \in Values /\ b \in Values /\ c \in Values
Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Invariants                                                              *)
----------------------------------------------------------------------------

TypeOK == a \in Values /\ b \in Values /\ c \in Values

ManifestTablesHold ==
    /\ \A v \in Values : TableNot[v] = Not(v)
    /\ \A x, y \in Values : TableAll[x \o y] = And(x, y) /\ TableAny[x \o y] = Or(x, y)

Commutative == And(a, b) = And(b, a) /\ Or(a, b) = Or(b, a)

Associative ==
    /\ And(a, And(b, c)) = And(And(a, b), c)
    /\ Or(a, Or(b, c)) = Or(Or(a, b), c)

Distributive ==
    /\ And(a, Or(b, c)) = Or(And(a, b), And(a, c))
    /\ Or(a, And(b, c)) = And(Or(a, b), Or(a, c))

Absorption == And(a, Or(a, b)) = a /\ Or(a, And(a, b)) = a

DeMorgan ==
    /\ Not(And(a, b)) = Or(Not(a), Not(b))
    /\ Not(Or(a, b)) = And(Not(a), Not(b))

DoubleNegation == Not(Not(a)) = a

\* Negating unknown leaves it unknown, so `not` never turns "could not ask"
\* into "yes".
NegationKeepsUnknown == (a = "U") <=> (Not(a) = "U")

\* The law that does not hold, and exactly where: `P or not P` is not
\* guaranteed, so nothing may assume it.
ExcludedMiddleFailsOnlyOnUnknown == (Or(a, Not(a)) # "T") <=> (a = "U")

\* A condition holds only when true: neither unknown nor its negation holds.
OnlyTrueHolds == Holds(a) <=> a = "T"

\* Early exits change no answer, for every group of every arity in bounds.
ShortCircuitIsEager ==
    \A s \in Groups :
        AllShort(s, "T") = AllEager(s) /\ AnyShort(s, "F") = AnyEager(s)

=============================================================================
