------------------------- MODULE RawRecordTakeover -------------------------
(***************************************************************************)
(* RawRecordOwnership with two duplicate executions of one payload against  *)
(* two reconcilers, with crashes on both sides. The protocol is the one     *)
(* module; this is a second instance of it, because the product of          *)
(* duplicate executions, reconciler takeovers and crashes is affordable at  *)
(* one span and nothing else's configuration affords it. This is where the  *)
(* confirm fence earns its keep: a collection decided by one reconciler,    *)
(* purged and dropped by another's takeover, a stale landing resurrecting   *)
(* the record while its writer may die at any step, and a late purge of the *)
(* first collector still in flight. Partial deletions and rewrites are      *)
(* RawRecordOwnership.cfg's; the three-collector races are                  *)
(* RawRecordReconcilers.cfg's.                                              *)
(***************************************************************************)
EXTENDS RawRecordOwnership
=============================================================================
