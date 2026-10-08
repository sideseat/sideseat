--------------------------- MODULE ContentBlockChain ---------------------------
(***************************************************************************)
(* How one content block is normalised: the canonical passthrough, the four *)
(* declared positions in order - `message_envelope` only for a message's    *)
(* own block, never for a value a tool returned - then the media and        *)
(* unknown fallbacks. Within a position the cases are tried by priority; a  *)
(* case that recognises a block and cannot build it declines to the next,   *)
(* and a recognising `unwrap` whose member cannot be normalised ends its     *)
(* position, leaving the original block to the positions after it.          *)
(*                                                                         *)
(* What is checked, each refutable by a change to the engine:              *)
(*                                                                         *)
(*   FirstSuccessWins     - the operational scan equals the declarative     *)
(*                          answer: the lowest-priority recognising case     *)
(*                          that builds, unless a recognising unwrap below   *)
(*                          it stopped the position.                         *)
(*   StopIsLocal          - a stopped position never answers, and does not   *)
(*                          stop the chain (a witness instance shows a later *)
(*                          position answering).                             *)
(*   DeclineIsTransparent - deleting a case that declines a block never      *)
(*                          changes that block's answer; deleting an unwrap  *)
(*                          that stopped can (witness).                      *)
(*   EnvelopesOnlyForMessages - a returned value is never answered by an      *)
(*                          envelope case.                                   *)
(*   Termination          - an unwrap descends to a strictly smaller value:  *)
(*                          every chain of members ends, and an unwrap of    *)
(*                          the whole block is refused by the compiler.      *)
(*   ExpectationsHold     - the manifest's hand-written answers.             *)
(*                                                                         *)
(* The instances are rendered from `instances/ContentBlockChain.json` by    *)
(* the Rust test `content_block_chain_instances`, which holds each position  *)
(* of the engine to `PositionAnswer` below.                                  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences, TLC

\* BEGIN GENERATED FROM instances/ContentBlockChain.json
ManifestPositions == <<"before_provider_formats", "message_envelope", "provider_formats", "after_provider_formats">>
ManifestBlocks == {
    [id |-> "plain", kind |-> "k1", strings |-> {"text"}, inner |-> "none", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "number_text", kind |-> "k1", strings |-> {}, inner |-> "none", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "alt_only", kind |-> "k1", strings |-> {"alt"}, inner |-> "none", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "wrapped", kind |-> "k2", strings |-> {"text"}, inner |-> "canonical_text", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "wrapped_empty", kind |-> "k2", strings |-> {"text"}, inner |-> "empty_string", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "wrapped_number", kind |-> "k2", strings |-> {}, inner |-> "number", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "double_wrapped", kind |-> "k2", strings |-> {}, inner |-> "wrapped", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "canonical_text", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> TRUE, fallback |-> FALSE],
    [id |-> "empty_string", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "number", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> FALSE, fallback |-> TRUE]
}
ManifestInstances == <<
    [id |-> "decline_then_build", cases |-> {[id |-> "c1", at |-> "provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "text"], [id |-> "c2", at |-> "provider_formats", prio |-> 20, form |-> "text", recognises |-> {"k1"}, member |-> "alt"]}, refused |-> FALSE, expect |-> << [block |-> "plain", message |-> "c1", returned |-> "c1"], [block |-> "number_text", message |-> "none", returned |-> "none"], [block |-> "alt_only", message |-> "c2", returned |-> "c2"] >>],
    [id |-> "unwrap_stops_its_position", cases |-> {[id |-> "u1", at |-> "provider_formats", prio |-> 10, form |-> "unwrap", recognises |-> {"k2"}, member |-> "inner"], [id |-> "c2", at |-> "provider_formats", prio |-> 20, form |-> "text", recognises |-> {"k2"}, member |-> "text"], [id |-> "c3", at |-> "after_provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k2"}, member |-> "text"]}, refused |-> FALSE, expect |-> << [block |-> "wrapped", message |-> "u1", returned |-> "u1"], [block |-> "wrapped_empty", message |-> "c3", returned |-> "c3"], [block |-> "wrapped_number", message |-> "u1", returned |-> "u1"], [block |-> "double_wrapped", message |-> "u1", returned |-> "u1"], [block |-> "canonical_text", message |-> "canonical", returned |-> "canonical"], [block |-> "number", message |-> "fallback", returned |-> "fallback"], [block |-> "empty_string", message |-> "none", returned |-> "none"] >>],
    [id |-> "envelope_and_provider", cases |-> {[id |-> "e1", at |-> "message_envelope", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "alt"], [id |-> "p1", at |-> "provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "text"]}, refused |-> FALSE, expect |-> << [block |-> "alt_only", message |-> "e1", returned |-> "none"], [block |-> "plain", message |-> "p1", returned |-> "p1"] >>],
    [id |-> "unwrap_of_the_whole_block", cases |-> {[id |-> "u1", at |-> "provider_formats", prio |-> 10, form |-> "unwrap", recognises |-> {"k2"}, member |-> "$"]}, refused |-> TRUE, expect |-> <<>>]
>>
\* END GENERATED FROM instances/ContentBlockChain.json

None == "none"

Range(s) == {s[k] : k \in DOMAIN s}
Inst(i) == ManifestInstances[i]
Blk(id) == CHOOSE b \in ManifestBlocks : b.id = id
Accepted == {i \in DOMAIN ManifestInstances : ~Inst(i).refused}

\* The compile-time refusal `UnwrapsWholeBlock`.
Refused(i) == \E c \in Inst(i).cases : c.form = "unwrap" /\ c.member = "$"
ASSUME \A i \in DOMAIN ManifestInstances : Inst(i).refused = Refused(i)

----------------------------------------------------------------------------
(* Termination. An unwrap reaches its member; the compiler refuses one that *)
(* reaches the block itself, so every chain of members is acyclic and ends  *)
(* within as many steps as there are blocks.                                *)
----------------------------------------------------------------------------
RECURSIVE Depth(_, _)
Depth(b, fuel) ==
    IF b.inner = None THEN 0
    ELSE IF fuel = 0 THEN 1000
    ELSE 1 + Depth(Blk(b.inner), fuel - 1)

Termination ==
    \A b \in ManifestBlocks : b.inner # b.id /\ Depth(b, Cardinality(ManifestBlocks)) < Cardinality(ManifestBlocks)
ASSUME Termination

----------------------------------------------------------------------------
(* One position, two ways.                                                 *)
----------------------------------------------------------------------------

Recognises(c, b) ==
    /\ ~b.foreign
    /\ b.kind \in c.recognises
    /\ (c.form = "unwrap" => b.inner # None)

Lowest(cs) == CHOOSE c \in cs : \A o \in cs : c.prio <= o.prio

\* The engine's scan: in priority order, a text case builds from a string member
\* or declines; a recognising unwrap answers with its member's normalisation, or
\* ends the position when there is none.
RECURSIVE Scan(_, _, _), Chain(_, _, _)
Scan(cs, b, rest) ==
    IF rest = {} THEN [kind |-> "nothing"]
    ELSE LET c == Lowest(rest)
         IN IF ~Recognises(c, b) THEN Scan(cs, b, rest \ {c})
            ELSE IF c.form = "text"
                 THEN IF c.member \in b.strings THEN [kind |-> "built", by |-> c.id]
                      ELSE Scan(cs, b, rest \ {c})
                 ELSE IF Chain(cs, Blk(b.inner), TRUE) # None
                      THEN [kind |-> "built", by |-> c.id]
                      ELSE [kind |-> "stopped", by |-> c.id]

PositionAnswer(cs, p, b) == Scan(cs, b, {c \in cs : c.at = p})

Order(message) ==
    SelectSeq(ManifestPositions, LAMBDA p : message \/ p # "message_envelope")

\* The whole chain: the passthrough, the positions in order, the fallbacks.
Chain(cs, b, message) ==
    IF b.canonical THEN "canonical"
    ELSE LET answering == {k \in DOMAIN Order(message) :
                              PositionAnswer(cs, Order(message)[k], b).kind = "built"}
         IN IF answering # {}
               THEN PositionAnswer(cs, Order(message)[CHOOSE k \in answering :
                                                        \A o \in answering : k <= o], b).by
               ELSE IF b.fallback THEN "fallback" ELSE None

\* Whether a recognising case can build, judged on its own.
Builds(cs, c, b) ==
    IF c.form = "text" THEN c.member \in b.strings
    ELSE Chain(cs, Blk(b.inner), TRUE) # None

\* The declarative answer: the lowest recognising case that builds, provided no
\* recognising unwrap below it failed to build.
Declared(cs, p, b) ==
    LET rec == {c \in cs : c.at = p /\ Recognises(c, b)}
        winners == {c \in rec : Builds(cs, c, b)
                       /\ ~\E u \in rec : u.prio < c.prio /\ u.form = "unwrap" /\ ~Builds(cs, u, b)}
        stoppers == {u \in rec : u.form = "unwrap" /\ ~Builds(cs, u, b)
                       /\ ~\E c \in rec : c.prio < u.prio /\ Builds(cs, c, b)}
    IN IF winners # {} THEN [kind |-> "built", by |-> Lowest(winners).id]
       ELSE IF stoppers # {} THEN [kind |-> "stopped", by |-> Lowest(stoppers).id]
       ELSE [kind |-> "nothing"]

----------------------------------------------------------------------------
(* The manifest's hand-written answers.                                    *)
----------------------------------------------------------------------------
ASSUME \A i \in Accepted :
    \A k \in DOMAIN Inst(i).expect :
        LET e == Inst(i).expect[k]
        IN /\ Chain(Inst(i).cases, Blk(e.block), TRUE) = e.message
           /\ Chain(Inst(i).cases, Blk(e.block), FALSE) = e.returned

\* A stopped position is local: some instance has a block that one position stops
\* and a later position answers.
ASSUME \E i \in Accepted, b \in ManifestBlocks :
    \E k, l \in DOMAIN Order(TRUE) :
        /\ k < l
        /\ PositionAnswer(Inst(i).cases, Order(TRUE)[k], b).kind = "stopped"
        /\ Chain(Inst(i).cases, b, TRUE) = PositionAnswer(Inst(i).cases, Order(TRUE)[l], b).by

\* Deleting an unwrap that stopped its position can change the answer: a stop is
\* not a decline.
ASSUME \E i \in Accepted, b \in ManifestBlocks : \E u \in Inst(i).cases :
    /\ u.form = "unwrap"
    /\ PositionAnswer(Inst(i).cases, u.at, b) = [kind |-> "stopped", by |-> u.id]
    /\ Chain(Inst(i).cases \ {u}, b, TRUE) # Chain(Inst(i).cases, b, TRUE)

----------------------------------------------------------------------------
(* The state is one instance, one block and one chain; there are no steps.  *)
----------------------------------------------------------------------------
VARIABLES inst, blk, message

vars == <<inst, blk, message>>

Init == inst \in Accepted /\ blk \in ManifestBlocks /\ message \in BOOLEAN
Spec == Init /\ [][UNCHANGED vars]_vars

Cases == Inst(inst).cases

TypeOK == inst \in Accepted /\ blk \in ManifestBlocks /\ message \in BOOLEAN

FirstSuccessWins ==
    \A p \in Range(ManifestPositions) : PositionAnswer(Cases, p, blk) = Declared(Cases, p, blk)

StopIsLocal ==
    \A k \in DOMAIN Order(message) :
        PositionAnswer(Cases, Order(message)[k], blk).kind = "stopped" =>
            Chain(Cases, blk, message) \notin {PositionAnswer(Cases, Order(message)[k], blk).by}

DeclineIsTransparent ==
    \A c \in Cases :
        (Recognises(c, blk) /\ c.form = "text" /\ ~Builds(Cases, c, blk)) =>
            Chain(Cases \ {c}, blk, message) = Chain(Cases, blk, message)

EnvelopesOnlyForMessages ==
    LET answer == Chain(Cases, blk, FALSE)
    IN \A c \in Cases : (c.id = answer) => c.at # "message_envelope"

\* The chain answers with a case that recognises the block, or with the engine's
\* own steps where the block is theirs.
AnswersComeFromRecognisers ==
    LET answer == Chain(Cases, blk, message)
    IN answer \in {"canonical", "fallback", None}
       \/ \E c \in Cases : c.id = answer /\ Recognises(c, blk) /\ Builds(Cases, c, blk)

============================================================================
