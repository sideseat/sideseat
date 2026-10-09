-------------------------- MODULE StagingRetirement --------------------------
(***************************************************************************)
(* How a durable-queue consumer tells a staged payload that was retired     *)
(* from one whose registration was lost - without keeping retired rows -   *)
(* and why a payload written again after it was retired cannot take over   *)
(* from a revision that arrived after it.                                   *)
(*                                                                          *)
(* An acknowledged export is a registry row plus a queue reference to it. A *)
(* reference legitimately outlives its row: redrive persists and retires a  *)
(* row whose message is still queued, a consumer can crash between         *)
(* retiring and acknowledging, a claimed message is processed twice, and a  *)
(* project deletion removes its rows. Keeping a tombstone per retirement    *)
(* would tell these apart, at one row per export kept for as long as a      *)
(* reference could be queued.                                               *)
(*                                                                          *)
(* Instead each registration takes the next value of a durable sequence,   *)
(* committed with its row, and the reference carries it. A power loss      *)
(* loses a suffix of commits, so a registration lost after its reference    *)
(* was published shows either as a sequence above the high-water mark, or  *)
(* - once later registrations have reused it - as that sequence now held   *)
(* by a different payload. A missing row with neither was retired or        *)
(* deleted.                                                                 *)
(*                                                                          *)
(* One double fault stays invisible: a lost sequence reused by a payload    *)
(* that is itself retired before the old reference is read. ReusedThen-     *)
(* Retired names it, and the invariants exclude nothing else. A sequence    *)
(* that is never reused (PostgreSQL's) removes it; SQLite's AUTOINCREMENT   *)
(* rolls back with the lost commit, so it can.                              *)
(*                                                                          *)
(* The payloads are revisions of one span, received in the order they are   *)
(* staged. Persisting and retiring are separate steps, as they are: a      *)
(* worker loads a payload, writes its revision, then settles it, and        *)
(* settling retires it only when its content is stored. The consumers      *)
(* stand for every worker - the inline requester and redrive take the same  *)
(* three steps without a reference - and nothing orders two workers'       *)
(* writes, so a worker that loaded an earlier revision can write it after  *)
(* another wrote a later one. The store answers with the revision at the    *)
(* latest instant. With ReceiptPrecedence the instant is the payload's      *)
(* receipt, so a copy written late is superseded at once and settles as     *)
(* superseded; without it the instant is the write's, and LaterReceiptWins *)
(* fails: the late copy takes over from the revision that arrived after it. *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS Payloads, Consumers, WithPowerLoss, ReceiptPrecedence, MaxWrites

VARIABLES
    row,        \* payload -> registry state
    seq,        \* payload -> the sequence its registration took (0 = none)
    hw,         \* the registry's sequence high-water mark
    queue,      \* payloads with an unacknowledged reference
    pc,         \* each consumer's message in hand and its next step
    verdict,    \* payload -> what a consumer concluded on reading its reference
    received,   \* payload -> its receipt order (0 = not yet received)
    written,    \* every revision stored: [p, at]
    \* Ghost facts the protocol cannot read, used only by the invariants.
    retired,    \* payloads retired at some point
    dropped,    \* payloads whose project was deleted
    lost,       \* payloads whose registration a power loss rolled back
    tail        \* the registrations committed since the last other commit, oldest first

vars == <<row, seq, hw, queue, pc, verdict, received, written, retired, dropped, lost, tail>>

Idle == [p |-> "none", step |-> "idle"]
MaxSeq == Cardinality(Payloads) + 1

TypeOK ==
    /\ row \in [Payloads -> {"none", "live", "gone"}]
    /\ seq \in [Payloads -> 0..MaxSeq]
    /\ hw \in 0..MaxSeq
    /\ queue \subseteq Payloads
    /\ \A c \in Consumers :
        pc[c] = Idle \/ (pc[c].p \in Payloads /\ pc[c].step \in {"read", "write", "settle", "ack"})
    /\ verdict \in [Payloads -> {"none", "processed", "finished", "anomaly"}]
    /\ received \in [Payloads -> 0..Cardinality(Payloads)]
    /\ retired \subseteq Payloads /\ dropped \subseteq Payloads /\ lost \subseteq Payloads

Init ==
    /\ row = [p \in Payloads |-> "none"]
    /\ seq = [p \in Payloads |-> 0]
    /\ hw = 0
    /\ queue = {}
    /\ pc = [c \in Consumers |-> Idle]
    /\ verdict = [p \in Payloads |-> "none"]
    /\ received = [p \in Payloads |-> 0]
    /\ written = {}
    /\ retired = {} /\ dropped = {} /\ lost = {}
    /\ tail = <<>>

\* The live row holding a sequence value, if any.
Holder(s) == {q \in Payloads : row[q] = "live" /\ seq[q] = s}

----------------------------------------------------------------------------
(* The span's revisions: the store answers with the latest by instant.     *)
(* Instants tie only for two writes of one payload under ReceiptPrecedence, *)
(* which are the same revision.                                            *)

Written(p) == \E r \in written : r.p = p

WinnerRecord == CHOOSE r \in written : \A o \in written : o.at <= r.at

Winner == IF written = {} THEN "none" ELSE WinnerRecord.p

\* Write p's revision: at its receipt with ReceiptPrecedence, at the write's own instant without. Writing a
\* revision again at its receipt stores nothing new, so only distinct revisions count towards the bound.
WriteRevision(p) ==
    /\ LET r == [p |-> p, at |-> IF ReceiptPrecedence THEN received[p] ELSE Cardinality(written) + 1]
       IN  /\ r \in written \/ Cardinality(written) < MaxWrites
           /\ written' = written \cup {r}

\* What settling reads: p's content is stored and is the winner - or, with ReceiptPrecedence, a revision received
\* after it is, which supersedes it legitimately.
Confirmed(p) ==
    /\ Written(p)
    /\ \/ Winner = p
       \/ (ReceiptPrecedence /\ Winner # "none" /\ received[Winner] > received[p])

----------------------------------------------------------------------------

\* Staging: the row commits with the next sequence, then the reference is published and the export answered.
\* A payload whose registration was lost is not staged again: its exporter was already answered.
Stage(p) ==
    /\ row[p] = "none" /\ seq[p] = 0 /\ hw < MaxSeq
    /\ row' = [row EXCEPT ![p] = "live"]
    /\ seq' = [seq EXCEPT ![p] = hw + 1]
    /\ hw' = hw + 1
    /\ queue' = queue \cup {p}
    /\ received' = [received EXCEPT ![p] = Cardinality({q \in Payloads : received[q] # 0}) + 1]
    /\ tail' = Append(tail, p)
    /\ UNCHANGED <<pc, verdict, written, retired, dropped, lost>>

DeleteProject(p) ==
    /\ row[p] = "live"
    /\ row' = [row EXCEPT ![p] = "gone"]
    /\ dropped' = dropped \cup {p}
    /\ tail' = <<>>
    /\ UNCHANGED <<seq, hw, queue, pc, verdict, received, written, retired, lost>>

\* The defect under detection: a WAL loses a suffix of its commits. Losing the newest registration of the
\* trailing run models any such suffix, one commit at a time; a lost retirement only resurrects a live row,
\* which is redelivered work rather than loss, so it is not modelled.
PowerLoss ==
    /\ WithPowerLoss
    /\ tail # <<>>
    /\ LET p == tail[Len(tail)] IN
        /\ row' = [row EXCEPT ![p] = "none"]
        /\ hw' = hw - 1
        /\ lost' = lost \cup {p}
        /\ tail' = SubSeq(tail, 1, Len(tail) - 1)
        \* The reference keeps the sequence it was published with, the exporter its answer.
        /\ UNCHANGED <<seq, queue, pc, verdict, received, written, retired, dropped>>

Take(c, p) ==
    /\ pc[c] = Idle /\ p \in queue
    /\ pc' = [pc EXCEPT ![c] = [p |-> p, step |-> "read"]]
    /\ UNCHANGED <<row, seq, hw, queue, verdict, received, written, retired, dropped, lost, tail>>

\* Reading decides. A live row is persisted, then settled. A missing one is classified by its sequence.
Read(c) ==
    /\ pc[c].step = "read"
    /\ LET p == pc[c].p
           s == seq[p]
       IN  IF row[p] = "live"
           THEN /\ pc' = [pc EXCEPT ![c].step = "write"]
                /\ UNCHANGED verdict
           ELSE /\ verdict' = [verdict EXCEPT ![p] =
                        IF s > hw \/ Holder(s) # {} THEN "anomaly" ELSE "finished"]
                /\ pc' = [pc EXCEPT ![c].step = "ack"]
    /\ UNCHANGED <<row, seq, hw, queue, received, written, retired, dropped, lost, tail>>

CWrite(c) ==
    /\ pc[c].step = "write"
    /\ WriteRevision(pc[c].p)
    /\ pc' = [pc EXCEPT ![c].step = "settle"]
    /\ UNCHANGED <<row, seq, hw, queue, verdict, received, retired, dropped, lost, tail>>

\* Settled, the reference is acknowledged; still pending, it stays queued for another delivery.
CSettle(c) ==
    /\ pc[c].step = "settle"
    /\ LET p == pc[c].p IN
        IF row[p] = "live" /\ Confirmed(p)
        THEN /\ row' = [row EXCEPT ![p] = "gone"]
             /\ retired' = retired \cup {p}
             /\ verdict' = [verdict EXCEPT ![p] = "processed"]
             /\ tail' = <<>>
             /\ pc' = [pc EXCEPT ![c].step = "ack"]
        ELSE /\ pc' = [pc EXCEPT ![c] = Idle]
             /\ UNCHANGED <<row, retired, verdict, tail>>
    /\ UNCHANGED <<seq, hw, queue, received, written, dropped, lost>>

Ack(c) ==
    /\ pc[c].step = "ack"
    /\ queue' = queue \ {pc[c].p}
    /\ pc' = [pc EXCEPT ![c] = Idle]
    /\ UNCHANGED <<row, seq, hw, verdict, received, written, retired, dropped, lost, tail>>

\* A crash loses a worker's place; an unacknowledged reference stays queued and is redelivered.
Crash(c) ==
    /\ pc[c] # Idle
    /\ pc' = [pc EXCEPT ![c] = Idle]
    /\ UNCHANGED <<row, seq, hw, queue, verdict, received, written, retired, dropped, lost, tail>>

Next ==
    \/ PowerLoss
    \/ \E p \in Payloads : Stage(p) \/ DeleteProject(p)
    \/ \E c \in Consumers : Read(c) \/ CWrite(c) \/ CSettle(c) \/ Ack(c) \/ Crash(c)
    \/ \E c \in Consumers, p \in Payloads : Take(c, p)

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Invariants                                                               *)
(***************************************************************************)

\* The one undetectable case: p's lost sequence was reused by a payload that has since been retired or
\* deleted, so nothing at or above the mark distinguishes it.
ReusedThenRetired(p) ==
    \E q \in Payloads \ {p} : seq[q] = seq[p] /\ (q \in retired \/ q \in dropped)

\* A reference accepted as finished was finished - retired or deleted - unless the double fault hides it.
NoSilentLoss ==
    \A p \in Payloads :
        verdict[p] = "finished" => (p \in retired \/ p \in dropped \/ ReusedThenRetired(p))

\* An anomaly is a real loss: nothing legitimate is ever reported.
NoFalseAnomaly ==
    \A p \in Payloads : verdict[p] = "anomaly" => p \in lost

\* Without power loss nothing is ever reported, and every finished reference was finished.
CleanRunsAreClean ==
    ~WithPowerLoss => \A p \in Payloads : verdict[p] # "anomaly"

\* Once a revision received later is stored, an earlier one is never the span's winner again - however late a
\* worker writes the copy of it that it loaded.
LaterReceiptWins ==
    \A p, q \in Payloads :
        (received[p] # 0 /\ received[q] > received[p] /\ Written(q)) => Winner # p

\* A payload is retired only once its content is stored: superseded is settled, unwritten is not.
RetiredWasWritten ==
    \A p \in retired : Written(p)
=============================================================================
