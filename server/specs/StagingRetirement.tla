-------------------------- MODULE StagingRetirement --------------------------
(***************************************************************************)
(* How a durable-queue consumer tells a staged payload that was retired     *)
(* from one whose registration was lost - without keeping retired rows.     *)
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
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS Payloads, Consumers, WithPowerLoss

VARIABLES
    row,        \* payload -> registry state
    seq,        \* payload -> the sequence its registration took (0 = none)
    hw,         \* the registry's sequence high-water mark
    queue,      \* payloads with an unacknowledged reference
    pc,         \* each consumer's message in hand and its next step
    verdict,    \* payload -> what a consumer concluded on reading its reference
    \* Ghost facts the protocol cannot read, used only by the invariants.
    retired,    \* payloads retired at some point
    dropped,    \* payloads whose project was deleted
    lost,       \* payloads whose registration a power loss rolled back
    tail        \* the registrations committed since the last other commit, oldest first

vars == <<row, seq, hw, queue, pc, verdict, retired, dropped, lost, tail>>

Idle == [p |-> "none", step |-> "idle"]
MaxSeq == Cardinality(Payloads) + 1

TypeOK ==
    /\ row \in [Payloads -> {"none", "live", "gone"}]
    /\ seq \in [Payloads -> 0..MaxSeq]
    /\ hw \in 0..MaxSeq
    /\ queue \subseteq Payloads
    /\ \A c \in Consumers : pc[c] = Idle \/ (pc[c].p \in Payloads /\ pc[c].step \in {"read", "ack"})
    /\ verdict \in [Payloads -> {"none", "processed", "finished", "anomaly"}]
    /\ retired \subseteq Payloads /\ dropped \subseteq Payloads /\ lost \subseteq Payloads

Init ==
    /\ row = [p \in Payloads |-> "none"]
    /\ seq = [p \in Payloads |-> 0]
    /\ hw = 0
    /\ queue = {}
    /\ pc = [c \in Consumers |-> Idle]
    /\ verdict = [p \in Payloads |-> "none"]
    /\ retired = {} /\ dropped = {} /\ lost = {}
    /\ tail = <<>>

\* The live row holding a sequence value, if any.
Holder(s) == {q \in Payloads : row[q] = "live" /\ seq[q] = s}

\* Staging: the row commits with the next sequence, then the reference is published and the export answered.
\* A payload whose registration was lost is not staged again: its exporter was already answered.
Stage(p) ==
    /\ row[p] = "none" /\ seq[p] = 0 /\ hw < MaxSeq
    /\ row' = [row EXCEPT ![p] = "live"]
    /\ seq' = [seq EXCEPT ![p] = hw + 1]
    /\ hw' = hw + 1
    /\ queue' = queue \cup {p}
    /\ tail' = Append(tail, p)
    /\ UNCHANGED <<pc, verdict, retired, dropped, lost>>

\* Retirement deletes the row. Redrive does it whatever the queue holds.
Redrive(p) ==
    /\ row[p] = "live"
    /\ row' = [row EXCEPT ![p] = "gone"]
    /\ retired' = retired \cup {p}
    /\ tail' = <<>>
    /\ UNCHANGED <<seq, hw, queue, pc, verdict, dropped, lost>>

DeleteProject(p) ==
    /\ row[p] = "live"
    /\ row' = [row EXCEPT ![p] = "gone"]
    /\ dropped' = dropped \cup {p}
    /\ tail' = <<>>
    /\ UNCHANGED <<seq, hw, queue, pc, verdict, retired, lost>>

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
        \* The reference keeps the sequence it was published with.
        /\ UNCHANGED <<seq, queue, pc, verdict, retired, dropped>>

Take(c, p) ==
    /\ pc[c] = Idle /\ p \in queue
    /\ pc' = [pc EXCEPT ![c] = [p |-> p, step |-> "read"]]
    /\ UNCHANGED <<row, seq, hw, queue, verdict, retired, dropped, lost, tail>>

\* Reading decides. A live row is persisted and retired. A missing one is classified by its sequence.
Read(c) ==
    /\ pc[c].step = "read"
    /\ LET p == pc[c].p
           s == seq[p]
       IN  IF row[p] = "live"
           THEN /\ row' = [row EXCEPT ![p] = "gone"]
                /\ retired' = retired \cup {p}
                /\ verdict' = [verdict EXCEPT ![p] = "processed"]
                /\ tail' = <<>>
           ELSE /\ verdict' = [verdict EXCEPT ![p] =
                        IF s > hw \/ Holder(s) # {} THEN "anomaly" ELSE "finished"]
                /\ UNCHANGED <<row, retired, tail>>
    /\ pc' = [pc EXCEPT ![c].step = "ack"]
    /\ UNCHANGED <<seq, hw, queue, dropped, lost>>

Ack(c) ==
    /\ pc[c].step = "ack"
    /\ queue' = queue \ {pc[c].p}
    /\ pc' = [pc EXCEPT ![c] = Idle]
    /\ UNCHANGED <<row, seq, hw, verdict, retired, dropped, lost, tail>>

\* A crash loses the consumer's place; an unacknowledged reference stays queued and is redelivered.
Crash(c) ==
    /\ pc[c] # Idle
    /\ pc' = [pc EXCEPT ![c] = Idle]
    /\ UNCHANGED <<row, seq, hw, queue, verdict, retired, dropped, lost, tail>>

Next ==
    \/ PowerLoss
    \/ \E p \in Payloads : Stage(p) \/ Redrive(p) \/ DeleteProject(p)
    \/ \E c \in Consumers : Read(c) \/ Ack(c) \/ Crash(c)
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
=============================================================================
