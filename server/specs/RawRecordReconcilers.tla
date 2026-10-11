------------------------ MODULE RawRecordReconcilers ------------------------
(***************************************************************************)
(* RawRecordOwnership with three reconcilers racing each other and one      *)
(* ingest. The protocol is the one module; this is a second instance of it, *)
(* because the interleavings of several reconcilers and of several ingests  *)
(* are each affordable to exhaust and their product is not. Three           *)
(* collectors cover what two cannot: a decision purged by its collector, a  *)
(* takeover of the same decision, and a third collection in flight, all at  *)
(* once, over partial deletions and rewrites. Duplicate executions are      *)
(* RawRecordTakeover.cfg's; backup and restore under a hold are             *)
(* RawRecordRestore.cfg's.                                                  *)
(***************************************************************************)
EXTENDS RawRecordOwnership
=============================================================================
