-------------------------- MODULE MediaIncarnation --------------------------
(***************************************************************************)
(* One stored file - one project's content hash - and why its bytes are    *)
(* stored under a name for each incarnation of its row rather than under    *)
(* the hash alone.                                                          *)
(*                                                                          *)
(* A file's row counts the references to it. An ingest associates the file *)
(* - adding a reference, or creating the row - then writes the bytes and is *)
(* acknowledged; a trace's deletion releases its reference. A collector     *)
(* claims a row nothing references, deletes the bytes, then the row. A      *)
(* claim that looks abandoned is taken over by another collector, and      *)
(* nothing stops the first from waking afterwards and taking the step it   *)
(* was about to take. Each row takes the next value of a sequence that is   *)
(* never reused, its incarnation, and the bytes are named by it.           *)
(*                                                                          *)
(* With Incarnations FALSE every incarnation shares the hash's one name,    *)
(* and TLC finds AckedReadable broken: the file collected by the second     *)
(* collector, ingested again and acknowledged, and the first collector's   *)
(* late delete removes the new bytes. With it, the late delete names an     *)
(* incarnation that is gone.                                               *)
(*                                                                          *)
(* A restore takes the row and the sequence back to a backup, and keeps     *)
(* the blob store's objects, whose recovery point is their own: names the   *)
(* lost timeline wrote survive it. The repair gives a restored row the      *)
(* bytes of any incarnation of its hash and deletes the others; with        *)
(* Advance it also moves the sequence past every incarnation stored. With   *)
(* CollectorsStopped, as the procedure requires, no collector of the lost   *)
(* timeline outlives the restore.                                           *)
(*                                                                          *)
(* With Lease the server's collectors outlive it, but run only under its   *)
(* store lease: a pass enters only while the lease is held, its delete is  *)
(* taken only while it still is - the check and the statement one step,   *)
(* the residual a lease cannot close being a statement already in flight - *)
(* and a pass that finds the lease lost gives up. The repair restores only *)
(* while no lease is held. A lost lease is taken again closed: with Drain   *)
(* it opens only once no pass entered before the loss is still running;    *)
(* without, at once, and a pass that claimed before the restore deletes    *)
(* after it.                                                               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS Collectors, Ingests, MaxIncarnation, Incarnations, Advance, CollectorsStopped, Lease, Drain

VARIABLES
    row,      \* the file's row: present, its incarnation, its references, and the collector holding its claim
    next,     \* the incarnation sequence's next value
    objects,  \* the names whose bytes are stored
    cpc,      \* each collector's step, and the incarnation it claimed
    ipc,      \* each ingest's step, and the incarnation it associated
    acked,    \* ghost: the ingests acknowledged whose reference is not released
    backup,   \* the transactional store's one backup, once taken: its row, sequence, and references
    restored, \* whether the backup was restored
    lease     \* the server's store lease: "held", "lost", or "draining" - taken again, its fence still closed

vars == <<row, next, objects, cpc, ipc, acked, backup, restored, lease>>

NoClaim == "none"
Absent == [present |-> FALSE, inc |-> 0, refs |-> 0, claim |-> NoClaim]
Idle == [step |-> "idle", inc |-> 0]
NoBackup == [taken |-> FALSE, row |-> Absent, next |-> 1, acked |-> {}, ipc |-> [i \in Ingests |-> Idle]]

Max(S) == CHOOSE x \in S : \A y \in S : x >= y

\* The name an incarnation's bytes are stored under.
Name(inc) == IF Incarnations THEN inc ELSE 0

TypeOK ==
    /\ row.present \in BOOLEAN /\ row.inc \in 0..MaxIncarnation /\ row.refs \in 0..Cardinality(Ingests)
    /\ row.claim \in Collectors \cup {NoClaim}
    /\ next \in 1..(MaxIncarnation + 1)
    /\ objects \subseteq 0..MaxIncarnation
    /\ \A c \in Collectors : cpc[c].step \in {"idle", "claimed", "bytes", "done"}
    /\ \A i \in Ingests : ipc[i].step \in {"idle", "associated", "written", "referenced", "acked", "released", "missing"}
    /\ acked \subseteq Ingests
    /\ restored \in BOOLEAN
    /\ lease \in {"held", "lost", "draining"}

\* Whether a collector may act: always without a lease; with one, only while it is held and its fence open.
Permitted == ~Lease \/ lease = "held"

\* A collector between entering its pass and leaving it.
InPass(c) == cpc[c].step \in {"claimed", "bytes"}

Init ==
    /\ row = Absent
    /\ next = 1
    /\ objects = {}
    /\ cpc = [c \in Collectors |-> [step |-> "idle", inc |-> 0]]
    /\ ipc = [i \in Ingests |-> [step |-> "idle", inc |-> 0]]
    /\ acked = {}
    /\ backup = NoBackup
    /\ restored = FALSE
    /\ lease = "held"

----------------------------------------------------------------------------
(* Ingests *)

\* Associate the file: a reference on the row there is, or a new row with the next incarnation. A claimed row
\* cannot be referenced until its claim is cleared.
Associate(i) ==
    /\ ipc[i].step = "idle"
    /\ row.claim = NoClaim
    /\ IF row.present
       THEN /\ row' = [row EXCEPT !.refs = @ + 1]
            /\ ipc' = [ipc EXCEPT ![i] = [step |-> "associated", inc |-> row.inc]]
            /\ UNCHANGED next
       ELSE /\ next <= MaxIncarnation
            /\ row' = [present |-> TRUE, inc |-> next, refs |-> 1, claim |-> NoClaim]
            /\ ipc' = [ipc EXCEPT ![i] = [step |-> "associated", inc |-> next]]
            /\ next' = next + 1
    /\ UNCHANGED <<objects, cpc, acked, backup, restored, lease>>

\* Write the bytes under the associated incarnation's name.
Write(i) ==
    /\ ipc[i].step = "associated"
    /\ objects' = objects \cup {Name(ipc[i].inc)}
    /\ ipc' = [ipc EXCEPT ![i].step = "written"]
    /\ UNCHANGED <<row, next, cpc, acked, backup, restored, lease>>

Ack(i) ==
    /\ ipc[i].step = "written"
    /\ ipc' = [ipc EXCEPT ![i].step = "acked"]
    /\ acked' = acked \cup {i}
    /\ UNCHANGED <<row, next, objects, cpc, backup, restored, lease>>

\* A reference that arrived already formed: no bytes to write, only an association with the row there is.
Reference(i) ==
    /\ ipc[i].step = "idle"
    /\ row.present /\ row.claim = NoClaim
    /\ row' = [row EXCEPT !.refs = @ + 1]
    /\ ipc' = [ipc EXCEPT ![i] = [step |-> "referenced", inc |-> row.inc]]
    /\ UNCHANGED <<next, objects, cpc, acked, backup, restored, lease>>

\* Acknowledged only once the bytes of the incarnation it is associated with are found: a check before the
\* association can see an incarnation collected since, the association then being a new one without bytes.
AckReference(i) ==
    /\ ipc[i].step = "referenced"
    /\ Name(ipc[i].inc) \in objects
    /\ ipc' = [ipc EXCEPT ![i].step = "acked"]
    /\ acked' = acked \cup {i}
    /\ UNCHANGED <<row, next, objects, cpc, backup, restored, lease>>

\* The trace that held the reference is deleted.
Release(i) ==
    /\ ipc[i].step = "acked"
    /\ row.present /\ row.inc = ipc[i].inc /\ row.refs > 0
    /\ row' = [row EXCEPT !.refs = @ - 1]
    /\ ipc' = [ipc EXCEPT ![i].step = "released"]
    /\ acked' = acked \ {i}
    /\ UNCHANGED <<next, objects, cpc, backup, restored, lease>>

----------------------------------------------------------------------------
(* Collectors *)

\* Claim a row nothing references - or take over a claim another collector holds, as an abandoned-looking one is.
Claim(c) ==
    /\ cpc[c].step = "idle"
    /\ Permitted
    /\ row.present /\ row.refs = 0 /\ row.claim # c
    /\ row' = [row EXCEPT !.claim = c]
    /\ cpc' = [cpc EXCEPT ![c] = [step |-> "claimed", inc |-> row.inc]]
    /\ UNCHANGED <<next, objects, ipc, acked, backup, restored, lease>>

\* The step after the claim, taken however long the collector paused: delete the claimed incarnation's bytes.
DeleteBytes(c) ==
    /\ cpc[c].step = "claimed"
    /\ Permitted
    /\ objects' = objects \ {Name(cpc[c].inc)}
    /\ cpc' = [cpc EXCEPT ![c].step = "bytes"]
    /\ UNCHANGED <<row, next, ipc, acked, backup, restored, lease>>

\* Delete the row, only while this collector still holds its claim on that incarnation and nothing references it.
DeleteRow(c) ==
    /\ cpc[c].step = "bytes"
    /\ IF row.present /\ row.claim = c /\ row.inc = cpc[c].inc /\ row.refs = 0
       THEN row' = Absent
       ELSE UNCHANGED row
    /\ cpc' = [cpc EXCEPT ![c] = [step |-> "idle", inc |-> 0]]
    /\ UNCHANGED <<next, objects, ipc, acked, backup, restored, lease>>

\* A pass that finds the lease lost before its delete gives up, holding nothing: its claim is the stale takeover's.
GiveUp(c) ==
    /\ Lease /\ lease # "held"
    /\ cpc[c].step = "claimed"
    /\ cpc' = [cpc EXCEPT ![c] = [step |-> "idle", inc |-> 0]]
    /\ UNCHANGED <<row, next, objects, ipc, acked, backup, restored, lease>>

----------------------------------------------------------------------------
(* The store lease *)

\* Renewals stop landing - a failover, a slow disk - for long enough that the lease is counted lost.
Lose ==
    /\ Lease /\ lease = "held"
    /\ lease' = "lost"
    /\ UNCHANGED <<row, next, objects, cpc, ipc, acked, backup, restored>>

\* Taken again, with the fence still closed.
Retake ==
    /\ lease = "lost"
    /\ lease' = "draining"
    /\ UNCHANGED <<row, next, objects, cpc, ipc, acked, backup, restored>>

\* The fence opens - with Drain, only once no pass entered before the loss is still running.
Reopen ==
    /\ lease = "draining"
    /\ Drain => \A c \in Collectors : ~InPass(c)
    /\ lease' = "held"
    /\ UNCHANGED <<row, next, objects, cpc, ipc, acked, backup, restored>>

----------------------------------------------------------------------------
(* Backup and restore *)

\* The transactional store is backed up; the blob store is not part of it.
Backup ==
    /\ ~backup.taken
    /\ backup' = [taken |-> TRUE, row |-> row, next |-> next, acked |-> acked, ipc |-> ipc]
    /\ UNCHANGED <<row, next, objects, cpc, ipc, acked, restored, lease>>

\* The backup is restored and repaired. Ingests in flight are lost with the timeline, and the repair settles the
\* row's references to the restored ones. A restored reference is acknowledged again only where the repair finds
\* its bytes; where it does not, it is reported missing, which is the restore's residual rather than this model's.
Restore ==
    /\ backup.taken /\ ~restored
    \* The repair refuses while a server's lease is live.
    /\ Lease => lease = "lost"
    /\ LET adopted == IF backup.row.present /\ objects # {} THEN {Name(backup.row.inc)} ELSE objects
           readable == backup.row.present /\ Name(backup.row.inc) \in adopted
           kept == IF readable THEN backup.acked ELSE {}
       IN /\ objects' = adopted
          /\ row' = IF backup.row.present THEN [backup.row EXCEPT !.refs = Cardinality(kept)] ELSE backup.row
          \* The repair's first step, before adoption: past every incarnation stored, the lost timeline's included.
          /\ next' = IF Advance THEN Max({backup.next} \cup {o + 1 : o \in objects}) ELSE backup.next
          /\ acked' = kept
          /\ ipc' = [i \in Ingests |->
                        IF i \in kept THEN backup.ipc[i]
                        ELSE IF i \in backup.acked THEN [step |-> "missing", inc |-> backup.ipc[i].inc]
                        ELSE Idle]
    /\ cpc' = IF CollectorsStopped THEN [c \in Collectors |-> Idle] ELSE cpc
    /\ restored' = TRUE
    /\ UNCHANGED <<backup, lease>>

Next ==
    \/ \E i \in Ingests : Associate(i) \/ Write(i) \/ Ack(i) \/ Reference(i) \/ AckReference(i) \/ Release(i)
    \/ \E c \in Collectors : Claim(c) \/ DeleteBytes(c) \/ DeleteRow(c) \/ GiveUp(c)
    \/ Lose \/ Retake \/ Reopen
    \/ Backup
    \/ Restore

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Invariants                                                               *)
(***************************************************************************)

\* An acknowledged reference reads its file: the row it holds is there, and so are the bytes its incarnation names.
AckedReadable ==
    \A i \in acked : row.present /\ row.inc = ipc[i].inc /\ Name(row.inc) \in objects

\* No incarnation is issued while an object is stored under its name - the lost timeline's included.
StoredNamesBelowNext == \A o \in objects : o < next

Symmetry == Permutations(Collectors) \cup Permutations(Ingests)
=============================================================================
