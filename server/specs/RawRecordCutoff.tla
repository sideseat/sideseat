--------------------------- MODULE RawRecordCutoff ---------------------------
(***************************************************************************)
(* Two raw records sharing one project epoch: the multi-record mapping      *)
(* RawRecordOwnership.tla argues informally, checked. That model has one    *)
(* record; across many, the epoch is one per-project sequence - the fencing *)
(* token every version is keyed under and every decision bumps - while each *)
(* record's collection state (pending decision, high-water mark, and the    *)
(* cutoff readers and the confirm fence apply) must be its own.             *)
(*                                                                         *)
(* Only what the mapping turns on is kept: a staged payload per record,    *)
(* registered as its pending writer until it settles, and an execution of  *)
(* it that reads the epoch, lands a version keyed under it and confirms     *)
(* through the fence (re-keying above the epoch when refused) - an          *)
(* execution that can outlive its payload's settlement, as a redriven       *)
(* duplicate does; ownership released; the collection decision; purges that *)
(* may run late, as takeovers do; and the drop into the high-water mark.     *)
(* `PerRecord` FALSE gives every record the project-wide cutoff - the       *)
(* greatest decision on any record - and breaks `OwnedVisible`: collecting  *)
(* one record hides the other's owned version. Removing the fence from      *)
(* `Confirm` breaks it too: a stale execution's version is owned under a    *)
(* decision some purge is entitled to.                                      *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS Records,  \* the records sharing the project
          MaxEpoch, \* bound on the project epoch, for the model checker
          PerRecord \* TRUE: each record's own cutoff; FALSE: one cutoff for the project

ASSUME Records # {} /\ MaxEpoch \in Nat /\ PerRecord \in BOOLEAN

None == -1

VARIABLES
    epoch,    \* the project epoch
    store,    \* store[r]: the epochs of r's stored versions
    owned,    \* owned[r]: r's durable owners exist (granted by the fence)
    reg,      \* reg[r]: r's staged payload is registered as its pending writer
    wpc,      \* wpc[r]: r's execution: "idle", "land", "confirm", "done"
    wep,      \* wep[r]: the epoch r's writer keyed its version under
    pending,  \* pending[r]: a decision awaiting its purge, or None
    hw,       \* hw[r]: the highest decision of r ever purged, or None
    purges    \* in-flight purges: <<record, decision epoch>>, possibly late, possibly repeated

vars == <<epoch, store, owned, reg, wpc, wep, pending, hw, purges>>

Max(a, b) == IF a > b THEN a ELSE b
OwnCutoff(r) == Max(hw[r], pending[r])
ProjectCutoff == LET cs == {OwnCutoff(r) : r \in Records} IN CHOOSE c \in cs : \A d \in cs : d <= c
Cutoff(r) == IF PerRecord THEN OwnCutoff(r) ELSE ProjectCutoff

\* What every reader sees of r: the versions keyed above its cutoff.
Visible(r) == {e \in store[r] : e > Cutoff(r)}

TypeOK ==
    /\ epoch \in 0..MaxEpoch
    /\ \A r \in Records : store[r] \subseteq 0..MaxEpoch /\ owned[r] \in BOOLEAN
    /\ \A r \in Records : wpc[r] \in {"idle", "land", "confirm", "done"}

Init ==
    /\ epoch = 0
    /\ store = [r \in Records |-> {}]
    /\ owned = [r \in Records |-> FALSE]
    /\ reg = [r \in Records |-> FALSE]
    /\ wpc = [r \in Records |-> "idle"]
    /\ wep = [r \in Records |-> 0]
    /\ pending = [r \in Records |-> None]
    /\ hw = [r \in Records |-> None]
    /\ purges = {}

\* A delivery registers its payload and an execution reads the project epoch; the version lands later under
\* it, whatever was decided meanwhile.
Look(r) ==
    /\ wpc[r] \in {"idle", "done"}
    /\ ~owned[r]
    /\ reg' = [reg EXCEPT ![r] = TRUE]
    /\ wpc' = [wpc EXCEPT ![r] = "land"]
    /\ wep' = [wep EXCEPT ![r] = epoch]
    /\ UNCHANGED <<epoch, store, owned, pending, hw, purges>>

\* The payload settles - by this execution once confirmed, or by a duplicate at any time - and its
\* registration goes. An execution still running carries on unregistered.
Settle(r) ==
    /\ reg[r]
    /\ reg' = [reg EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<epoch, store, owned, wpc, wep, pending, hw, purges>>

Land(r) ==
    /\ wpc[r] = "land"
    /\ store' = [store EXCEPT ![r] = @ \cup {wep[r]}]
    /\ wpc' = [wpc EXCEPT ![r] = "confirm"]
    /\ UNCHANGED <<epoch, owned, reg, wep, pending, hw, purges>>

\* The fence: granted only above the cutoff; refused, the writer re-keys a version at the current epoch.
Confirm(r) ==
    /\ wpc[r] = "confirm"
    /\ IF wep[r] > Cutoff(r)
         THEN /\ owned' = [owned EXCEPT ![r] = TRUE]
              /\ wpc' = [wpc EXCEPT ![r] = "done"]
              /\ UNCHANGED <<store, wep>>
         ELSE /\ store' = [store EXCEPT ![r] = @ \cup {epoch}]
              /\ wep' = [wep EXCEPT ![r] = epoch]
              /\ UNCHANGED <<owned, wpc>>
    /\ UNCHANGED <<epoch, reg, pending, hw, purges>>

Release(r) ==
    /\ owned[r]
    /\ owned' = [owned EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<epoch, store, reg, wpc, wep, pending, hw, purges>>

\* No owner, no registered writer, no decision pending: recorded at the epoch, which it bumps. An execution that
\* outlived its payload's settlement does not block it - nothing records it.
Collect(r) ==
    /\ ~owned[r]
    /\ ~reg[r]
    /\ pending[r] = None
    /\ epoch < MaxEpoch
    /\ pending' = [pending EXCEPT ![r] = epoch]
    /\ epoch' = epoch + 1
    /\ purges' = purges \cup {<<r, epoch>>}
    /\ UNCHANGED <<store, owned, reg, wpc, wep, hw>>

\* A purge of a decision: every version of its record keyed at or below the decision's epoch. It may run at any
\* time after the decision - before or after the drop, more than once - as a slow collector or a takeover does.
Purge(p) ==
    /\ p \in purges
    /\ store' = [store EXCEPT ![p[1]] = {e \in @ : e > p[2]}]
    /\ UNCHANGED <<epoch, owned, reg, wpc, wep, pending, hw, purges>>

\* The purge done, the decision folds into the record's high-water mark. A straggling purge stays possible.
Drop(r) ==
    /\ pending[r] # None
    /\ hw' = [hw EXCEPT ![r] = pending[r]]
    /\ pending' = [pending EXCEPT ![r] = None]
    /\ store' = [store EXCEPT ![r] = {e \in @ : e > pending[r]}]
    /\ UNCHANGED <<epoch, owned, reg, wpc, wep, purges>>

Next ==
    \E r \in Records : Look(r) \/ Settle(r) \/ Land(r) \/ Confirm(r) \/ Release(r) \/ Collect(r) \/ Drop(r)
    \/ \E p \in purges : Purge(p)

Spec == Init /\ [][Next]_vars

\* An owned record is readable: its owners' version is visible, and no purge - however late - removed it.
OwnedVisible == \A r \in Records : owned[r] => Visible(r) # {}

Symmetry == Permutations(Records)
=============================================================================
