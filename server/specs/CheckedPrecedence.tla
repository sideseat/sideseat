------------------------- MODULE CheckedPrecedence -------------------------
(***************************************************************************)
(* **Which of several matching clauses answers**, in every ordered arena of *)
(* the rule language, and what the compilers refuse.                       *)
(*                                                                         *)
(* Every arena - detection, the two classifications, contending message    *)
(* rules, one content-chain position, tool shapes, event categories, each  *)
(* ordered member question - orders its clauses by an integer `priority`, *)
(* lowest first (`server/crates/domain/src/rules/precedence.rs`). Three    *)
(* things are refused at compile time, in this order: two clauses sharing *)
(* a priority; a `supersedes` edge that names nothing, names its source,  *)
(* repeats, or points at a clause tried first; and a clause an earlier one *)
(* always satisfies. `supersedes` is never executed.                       *)
(*                                                                         *)
(* It once was: detection picked the lowest-ranked matching clause that no *)
(* matching clause transitively superseded (`RetiredWinner` below), and two *)
(* shipped edges pointed at clauses ranked ahead of their sources - a       *)
(* preference relation no total order states. This model is the argument  *)
(* for retiring that: wherever the edges agree with the priorities, the    *)
(* retired resolution and the priority give the same answer, and deleting *)
(* an agreeing edge changes nothing (`AgreeingEdgesAreInert`,              *)
(* `EdgeDeletionChangesNothing`).                                          *)
(*                                                                         *)
(* A clause's conditions are abstracted to the set of spans it matches, as *)
(* `OrderedResolution` did before it: whether the engine's predicates      *)
(* establish that set is the predicate language's question. The instances *)
(* are of two kinds, both checked by every invariant:                      *)
(*                                                                         *)
(*   - every instance within `ManifestBounds` - each priority function, each *)
(*     match table, each edge list up to `ManifestMaxEdges` long;           *)
(*   - the hand-written cases of `server/specs/instances/CheckedPrecedence.json`, *)
(*     with the diagnostic and the winners their author expected.          *)
(*                                                                         *)
(* The Rust property test `checked_precedence_instances` reads the same    *)
(* manifest and holds the production compilers (detection and observation *)
(* types) and both resolutions to the same expectations and the same      *)
(* bounded space, and it regenerates the section between the markers below *)
(* (`UPDATE_SPECS=1`), failing while it is stale - so the model, the       *)
(* manifest and the engine cannot drift apart. Finite coverage is a bounded *)
(* check, not a proof over unrestricted inputs.                            *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, Sequences, TLC

\* BEGIN GENERATED FROM instances/CheckedPrecedence.json
ManifestIds == {"a", "b", "c"}
ManifestSpans == {"s1", "s2"}
ManifestPriorities == {-1, 0, 1}
ManifestMaxEdges == 2
ManifestInstances == {
    [id |-> "lowest_priority_wins",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2", "s3"},
     prio |-> ("a" :> 10 @@ "b" :> 20),
     edges |-> <<>>,
     matches |-> ("a" :> {"s1", "s2"} @@ "b" :> {"s2", "s3"}),
     diagnostic |-> "none",
     winners |-> ("s1" :> "a" @@ "s2" :> "a" @@ "s3" :> "b"),
     retired |-> ("s1" :> "a" @@ "s2" :> "a" @@ "s3" :> "b")],
    [id |-> "an_agreeing_edge_changes_no_answer",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2", "s3"},
     prio |-> ("a" :> 10 @@ "b" :> 20),
     edges |-> << <<"a", "b">> >>,
     matches |-> ("a" :> {"s1", "s2"} @@ "b" :> {"s2", "s3"}),
     diagnostic |-> "none",
     winners |-> ("s1" :> "a" @@ "s2" :> "a" @@ "s3" :> "b"),
     retired |-> ("s1" :> "a" @@ "s2" :> "a" @@ "s3" :> "b")],
    [id |-> "a_transitive_chain_of_agreeing_edges",
     ids |-> {"a", "b", "c"},
     spans |-> {"s1", "s2", "s3", "s4"},
     prio |-> ("a" :> 1 @@ "b" :> 2 @@ "c" :> 3),
     edges |-> << <<"a", "b">>, <<"b", "c">> >>,
     matches |-> ("a" :> {"s1", "s4"} @@ "b" :> {"s2", "s4"} @@ "c" :> {"s3", "s4"}),
     diagnostic |-> "none",
     winners |-> ("s1" :> "a" @@ "s2" :> "b" @@ "s3" :> "c" @@ "s4" :> "a"),
     retired |-> ("s1" :> "a" @@ "s2" :> "b" @@ "s3" :> "c" @@ "s4" :> "a")],
    [id |-> "an_edge_against_the_priorities_is_refused",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2", "s3"},
     prio |-> ("a" :> 10 @@ "b" :> 20),
     edges |-> << <<"b", "a">> >>,
     matches |-> ("a" :> {"s1", "s2"} @@ "b" :> {"s2", "s3"}),
     diagnostic |-> "UselessSupersedes",
     winners |-> ("s1" :> "a" @@ "s2" :> "a" @@ "s3" :> "b"),
     retired |-> ("s1" :> "a" @@ "s2" :> "b" @@ "s3" :> "b")],
    [id |-> "the_retired_cycle_of_preferences",
     ids |-> {"a", "b", "c"},
     spans |-> {"ab", "bc", "ca", "abc"},
     prio |-> ("a" :> 1 @@ "b" :> 2 @@ "c" :> 3),
     edges |-> << <<"c", "a">> >>,
     matches |-> ("a" :> {"ab", "ca", "abc"} @@ "b" :> {"ab", "bc", "abc"} @@ "c" :> {"bc", "ca", "abc"}),
     diagnostic |-> "UselessSupersedes",
     winners |-> ("ab" :> "a" @@ "abc" :> "a" @@ "bc" :> "b" @@ "ca" :> "a"),
     retired |-> ("ab" :> "a" @@ "abc" :> "b" @@ "bc" :> "b" @@ "ca" :> "c")],
    [id |-> "a_shared_priority_is_refused",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2"},
     prio |-> ("a" :> 5 @@ "b" :> 5),
     edges |-> <<>>,
     matches |-> ("a" :> {"s1"} @@ "b" :> {"s2"}),
     diagnostic |-> "DuplicatePriority",
     winners |-> <<>>,
     retired |-> <<>>],
    [id |-> "an_edge_naming_nothing_is_refused",
     ids |-> {"a"},
     spans |-> {"s1"},
     prio |-> ("a" :> 1),
     edges |-> << <<"a", "ghost">> >>,
     matches |-> ("a" :> {"s1"}),
     diagnostic |-> "UselessSupersedes",
     winners |-> ("s1" :> "a"),
     retired |-> ("s1" :> "a")],
    [id |-> "an_edge_to_itself_is_refused",
     ids |-> {"a"},
     spans |-> {"s1"},
     prio |-> ("a" :> 1),
     edges |-> << <<"a", "a">> >>,
     matches |-> ("a" :> {"s1"}),
     diagnostic |-> "UselessSupersedes",
     winners |-> ("s1" :> "a"),
     retired |-> ("s1" :> "a")],
    [id |-> "a_repeated_edge_is_refused",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2"},
     prio |-> ("a" :> 1 @@ "b" :> 2),
     edges |-> << <<"a", "b">>, <<"a", "b">> >>,
     matches |-> ("a" :> {"s1"} @@ "b" :> {"s2"}),
     diagnostic |-> "UselessSupersedes",
     winners |-> ("s1" :> "a" @@ "s2" :> "b"),
     retired |-> ("s1" :> "a" @@ "s2" :> "b")],
    [id |-> "a_clause_an_earlier_one_covers_is_refused",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2"},
     prio |-> ("a" :> 1 @@ "b" :> 2),
     edges |-> <<>>,
     matches |-> ("a" :> {"s1", "s2"} @@ "b" :> {"s2"}),
     diagnostic |-> "ShadowedRule",
     winners |-> ("s1" :> "a" @@ "s2" :> "a"),
     retired |-> ("s1" :> "a" @@ "s2" :> "a")],
    [id |-> "a_later_broader_clause_is_not_shadowed",
     ids |-> {"a", "b"},
     spans |-> {"s1", "s2"},
     prio |-> ("a" :> 1 @@ "b" :> 2),
     edges |-> << <<"a", "b">> >>,
     matches |-> ("a" :> {"s2"} @@ "b" :> {"s1", "s2"}),
     diagnostic |-> "none",
     winners |-> ("s1" :> "b" @@ "s2" :> "a"),
     retired |-> ("s1" :> "b" @@ "s2" :> "a")],
    [id |-> "signed_priorities_order_as_integers",
     ids |-> {"a", "b"},
     spans |-> {"s1"},
     prio |-> ("a" :> 1 @@ "b" :> -1),
     edges |-> << <<"b", "a">> >>,
     matches |-> ("a" :> {"s1"} @@ "b" :> {"s1"}),
     diagnostic |-> "ShadowedRule",
     winners |-> ("s1" :> "b"),
     retired |-> ("s1" :> "b")],
    [id |-> "nothing_matches",
     ids |-> {"a"},
     spans |-> {"s1", "s2"},
     prio |-> ("a" :> 1),
     edges |-> <<>>,
     matches |-> ("a" :> {"s1"}),
     diagnostic |-> "none",
     winners |-> ("s1" :> "a" @@ "s2" :> "none"),
     retired |-> ("s1" :> "a" @@ "s2" :> "none")]
}
\* END GENERATED FROM instances/CheckedPrecedence.json

None == "none"

\* Every edge list up to the bound, duplicates and self-edges included: the
\* refusals have to be reachable for the model to say anything about them.
EdgeLists == UNION {[1..n -> ManifestIds \X ManifestIds] : n \in 0..ManifestMaxEdges}

Enumerated ==
    [id : {"enumerated"},
     ids : {ManifestIds},
     spans : {ManifestSpans},
     prio : [ManifestIds -> ManifestPriorities],
     edges : EdgeLists,
     matches : [ManifestIds -> SUBSET ManifestSpans],
     diagnostic : {None},
     winners : {<<>>},
     retired : {<<>>}]

VARIABLES inst, span

vars == <<inst, span>>

----------------------------------------------------------------------------
(* What the compilers decide.                                              *)
----------------------------------------------------------------------------

SharedPriority(i) == \E a, b \in i.ids : a # b /\ i.prio[a] = i.prio[b]

DefectiveEdge(i, k) ==
    LET a == i.edges[k][1]
        b == i.edges[k][2]
    IN \/ a = b
       \/ b \notin i.ids
       \/ \E j \in 1..(k - 1) : i.edges[j] = i.edges[k]
       \/ i.prio[b] <= i.prio[a]

EdgeDefect(i) == \E k \in DOMAIN i.edges : DefectiveEdge(i, k)

\* Pairs <<earlier, later>> where the earlier clause holds wherever the later one does.
ShadowPairs(i) ==
    {p \in i.ids \X i.ids :
        p[1] # p[2] /\ i.prio[p[1]] < i.prio[p[2]] /\ i.matches[p[2]] \subseteq i.matches[p[1]]}

Diagnostic(i) ==
    IF SharedPriority(i) THEN "DuplicatePriority"
    ELSE IF EdgeDefect(i) THEN "UselessSupersedes"
    ELSE IF ShadowPairs(i) # {} THEN "ShadowedRule"
    ELSE None

----------------------------------------------------------------------------
(* The two resolutions.                                                    *)
----------------------------------------------------------------------------

Matching(i, s) == {r \in i.ids : s \in i.matches[r]}

Lowest(i, set) == CHOOSE r \in set : \A o \in set : i.prio[r] <= i.prio[o]

\* Today's: the lowest-priority matching clause.
Winner(i, s) == IF Matching(i, s) = {} THEN None ELSE Lowest(i, Matching(i, s))

\* The retired one: edges transitively closed, a clause beaten by a matching
\* clause drops out, the lowest of the rest answers - and the rank winner where
\* every match is beaten, which only a refused cycle allows.
RECURSIVE Closure(_, _)
Closure(edges, rounds) ==
    IF rounds = 0 THEN edges
    ELSE LET joined == {pq \in edges \X edges : pq[1][2] = pq[2][1]}
             grown == edges \cup {<<pq[1][1], pq[2][2]>> : pq \in joined}
         IN IF grown = edges THEN edges ELSE Closure(grown, rounds - 1)

EdgeSet(i) == {i.edges[k] : k \in DOMAIN i.edges}

Beats(i) == Closure(EdgeSet(i), Cardinality(i.ids) + 1)

Unbeaten(i, s) ==
    {r \in Matching(i, s) : ~\E o \in Matching(i, s) : <<o, r>> \in Beats(i)}

RetiredWinner(i, s) ==
    IF Matching(i, s) = {} THEN None
    ELSE IF Unbeaten(i, s) = {} THEN Lowest(i, Matching(i, s))
    ELSE Lowest(i, Unbeaten(i, s))

Drop(i, k) ==
    [i EXCEPT !.edges = [j \in 1..(Len(i.edges) - 1) |->
                            IF j < k THEN i.edges[j] ELSE i.edges[j + 1]]]

----------------------------------------------------------------------------
Init == /\ inst \in Enumerated \cup ManifestInstances
        /\ span \in inst.spans
Next == UNCHANGED vars
Spec == Init /\ [][Next]_vars

----------------------------------------------------------------------------
(* Invariants                                                              *)
----------------------------------------------------------------------------

TypeOK == span \in inst.spans /\ inst.ids # {}

\* Where no two clauses tie and every edge agrees with the priorities, the
\* retired resolution answers exactly as the priority does - so `supersedes`
\* can stop executing without changing any answer of an accepted asset.
AgreeingEdgesAreInert ==
    (~SharedPriority(inst) /\ ~EdgeDefect(inst))
        => RetiredWinner(inst, span) = Winner(inst, span)

\* And deleting any one agreeing edge changes neither what the compiler
\* decides nor what the retired resolution answered.
EdgeDeletionChangesNothing ==
    (~SharedPriority(inst) /\ ~EdgeDefect(inst))
        => \A k \in DOMAIN inst.edges :
               /\ Diagnostic(Drop(inst, k)) = Diagnostic(inst)
               /\ RetiredWinner(Drop(inst, k), span) = RetiredWinner(inst, span)

\* The shadow refusal removes nothing an author could rely on: a clause an
\* earlier one covers never wins.
ShadowRefusalIsSound ==
    ~SharedPriority(inst) => \A p \in ShadowPairs(inst) : Winner(inst, span) # p[2]

\* Nothing answers exactly where nothing matches.
NoneOnlyWhereNothingMatches ==
    ~SharedPriority(inst) => ((Winner(inst, span) = None) <=> (Matching(inst, span) = {}))

\* The winner matches, and nothing that matches is tried before it.
WinnerIsTheLowestMatch ==
    (~SharedPriority(inst) /\ Winner(inst, span) # None)
        => /\ span \in inst.matches[Winner(inst, span)]
           /\ \A o \in Matching(inst, span) : inst.prio[Winner(inst, span)] <= inst.prio[o]

\* The manifest's hand-written expectations.
ManifestHolds ==
    inst.id # "enumerated" =>
        /\ Diagnostic(inst) = inst.diagnostic
        /\ span \in DOMAIN inst.winners => Winner(inst, span) = inst.winners[span]
        /\ span \in DOMAIN inst.retired => RetiredWinner(inst, span) = inst.retired[span]

============================================================================
