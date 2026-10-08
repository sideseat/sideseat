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
(*                          envelope case, through any unwrap of it.         *)
(*   Termination          - an unwrap descends to a strictly smaller value:  *)
(*                          every chain of members ends, and an unwrap of    *)
(*                          the whole block is refused by the compiler.      *)
(*   ExpectationsHold     - the manifest's hand-written answers.             *)
(*                                                                         *)
(* The instances are rendered from `instances/ContentBlockChain.json` by    *)
(* the Rust test `content_block_chain_instances`, which holds each position  *)
(* of the engine to `PositionAnswer` below, and the production chain -       *)
(* `normalize_block_in`, unwrap recursion included - to the whole answer.    *)
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
    [id |-> "wrapped_alt", kind |-> "k2", strings |-> {}, inner |-> "alt_only", foreign |-> FALSE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "canonical_text", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> TRUE, fallback |-> FALSE],
    [id |-> "empty_string", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> FALSE, fallback |-> FALSE],
    [id |-> "number", kind |-> "", strings |-> {}, inner |-> "none", foreign |-> TRUE, canonical |-> FALSE, fallback |-> TRUE]
}
ManifestInstances == <<
    [id |-> "decline_then_build", cases |-> {[id |-> "c1", at |-> "provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "text"], [id |-> "c2", at |-> "provider_formats", prio |-> 20, form |-> "text", recognises |-> {"k1"}, member |-> "alt"]}, refused |-> FALSE, expect |-> << [block |-> "plain", message |-> "c1", returned |-> "c1"], [block |-> "number_text", message |-> "fallback", returned |-> "fallback"], [block |-> "alt_only", message |-> "c2", returned |-> "c2"] >>],
    [id |-> "unwrap_stops_its_position", cases |-> {[id |-> "u1", at |-> "provider_formats", prio |-> 10, form |-> "unwrap", recognises |-> {"k2"}, member |-> "inner"], [id |-> "c2", at |-> "provider_formats", prio |-> 20, form |-> "text", recognises |-> {"k2"}, member |-> "text"], [id |-> "c3", at |-> "after_provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k2"}, member |-> "text"]}, refused |-> FALSE, expect |-> << [block |-> "wrapped", message |-> "u1", returned |-> "u1"], [block |-> "wrapped_empty", message |-> "c3", returned |-> "c3"], [block |-> "wrapped_number", message |-> "u1", returned |-> "u1"], [block |-> "double_wrapped", message |-> "u1", returned |-> "u1"], [block |-> "canonical_text", message |-> "canonical", returned |-> "canonical"], [block |-> "number", message |-> "fallback", returned |-> "fallback"], [block |-> "empty_string", message |-> "none", returned |-> "none"] >>],
    [id |-> "envelope_and_provider", cases |-> {[id |-> "e1", at |-> "message_envelope", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "alt"], [id |-> "p1", at |-> "provider_formats", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "text"]}, refused |-> FALSE, expect |-> << [block |-> "alt_only", message |-> "e1", returned |-> "fallback"], [block |-> "plain", message |-> "p1", returned |-> "p1"] >>],
    [id |-> "returned_member_skips_envelopes", cases |-> {[id |-> "u1", at |-> "before_provider_formats", prio |-> 10, form |-> "unwrap", recognises |-> {"k2"}, member |-> "inner"], [id |-> "e1", at |-> "message_envelope", prio |-> 10, form |-> "text", recognises |-> {"k1"}, member |-> "alt"]}, refused |-> FALSE, expect |-> << [block |-> "wrapped_alt", message |-> "u1", returned |-> "u1"], [block |-> "alt_only", message |-> "e1", returned |-> "fallback"] >>],
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
\* ends the position when there is none. The member is the same kind of value as
\* the block around it - a message's or a returned one - so it is normalised
\* under the same `message`.
RECURSIVE Scan(_, _, _, _), Chain(_, _, _), Used(_, _, _)
Scan(cs, b, rest, message) ==
    IF rest = {} THEN [kind |-> "nothing"]
    ELSE LET c == Lowest(rest)
         IN IF ~Recognises(c, b) THEN Scan(cs, b, rest \ {c}, message)
            ELSE IF c.form = "text"
                 THEN IF c.member \in b.strings THEN [kind |-> "built", by |-> c.id]
                      ELSE Scan(cs, b, rest \ {c}, message)
                 ELSE IF Chain(cs, Blk(b.inner), message) # None
                      THEN [kind |-> "built", by |-> c.id]
                      ELSE [kind |-> "stopped", by |-> c.id]

PositionAnswer(cs, p, b, message) == Scan(cs, b, {c \in cs : c.at = p}, message)

Order(message) ==
    SelectSeq(ManifestPositions, LAMBDA p : message \/ p # "message_envelope")

\* The whole chain: the passthrough, the positions in order, the fallbacks. A
\* declared block is typed, so the unknown fallback answers it where no case does.
Chain(cs, b, message) ==
    IF b.canonical THEN "canonical"
    ELSE LET answering == {k \in DOMAIN Order(message) :
                              PositionAnswer(cs, Order(message)[k], b, message).kind = "built"}
         IN IF answering # {}
               THEN PositionAnswer(cs, Order(message)[CHOOSE k \in answering :
                                                        \A o \in answering : k <= o], b, message).by
               ELSE IF b.fallback \/ ~b.foreign THEN "fallback" ELSE None

\* Every case the answer is built through: the answering case and, for an unwrap,
\* every case its member's answer is built through.
Used(cs, b, message) ==
    LET answer == Chain(cs, b, message)
    IN IF answer \in {"canonical", "fallback", None} THEN {}
       ELSE LET c == CHOOSE c \in cs : c.id = answer
            IN IF c.form = "unwrap" THEN {answer} \cup Used(cs, Blk(b.inner), message)
               ELSE {answer}

\* Whether a recognising case can build, judged on its own.
Builds(cs, c, b, message) ==
    IF c.form = "text" THEN c.member \in b.strings
    ELSE Chain(cs, Blk(b.inner), message) # None

\* The declarative answer: the lowest recognising case that builds, provided no
\* recognising unwrap below it failed to build.
Declared(cs, p, b, message) ==
    LET rec == {c \in cs : c.at = p /\ Recognises(c, b)}
        winners == {c \in rec : Builds(cs, c, b, message)
                       /\ ~\E u \in rec : u.prio < c.prio /\ u.form = "unwrap" /\ ~Builds(cs, u, b, message)}
        stoppers == {u \in rec : u.form = "unwrap" /\ ~Builds(cs, u, b, message)
                       /\ ~\E c \in rec : c.prio < u.prio /\ Builds(cs, c, b, message)}
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
        /\ PositionAnswer(Inst(i).cases, Order(TRUE)[k], b, TRUE).kind = "stopped"
        /\ Chain(Inst(i).cases, b, TRUE) = PositionAnswer(Inst(i).cases, Order(TRUE)[l], b, TRUE).by

\* Deleting an unwrap that stopped its position can change the answer: a stop is
\* not a decline.
ASSUME \E i \in Accepted, b \in ManifestBlocks : \E u \in Inst(i).cases :
    /\ u.form = "unwrap"
    /\ PositionAnswer(Inst(i).cases, u.at, b, TRUE) = [kind |-> "stopped", by |-> u.id]
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
    \A p \in Range(ManifestPositions) :
        PositionAnswer(Cases, p, blk, message) = Declared(Cases, p, blk, message)

StopIsLocal ==
    \A k \in DOMAIN Order(message) :
        PositionAnswer(Cases, Order(message)[k], blk, message).kind = "stopped" =>
            Chain(Cases, blk, message) \notin {PositionAnswer(Cases, Order(message)[k], blk, message).by}

DeclineIsTransparent ==
    \A c \in Cases :
        (Recognises(c, blk) /\ c.form = "text" /\ ~Builds(Cases, c, blk, message)) =>
            Chain(Cases \ {c}, blk, message) = Chain(Cases, blk, message)

\* Through every unwrap of the answer, not only the outer case: a returned value's
\* member is a returned value too.
EnvelopesOnlyForMessages ==
    \A c \in Cases : c.id \in Used(Cases, blk, FALSE) => c.at # "message_envelope"

\* The chain answers with a case that recognises the block, or with the engine's
\* own steps where the block is theirs.
AnswersComeFromRecognisers ==
    LET answer == Chain(Cases, blk, message)
    IN answer \in {"canonical", "fallback", None}
       \/ \E c \in Cases : c.id = answer /\ Recognises(c, blk) /\ Builds(Cases, c, blk, message)

============================================================================
