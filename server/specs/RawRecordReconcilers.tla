------------------------ MODULE RawRecordReconcilers ------------------------
(***************************************************************************)
(* RawRecordOwnership with two reconcilers racing each other and one        *)
(* ingest, through a legal hold. The protocol is the one module; this is a  *)
(* second instance of it, because the interleavings of two reconcilers and  *)
(* of two ingests are each cheap to exhaust and their product is not.      *)
(* Restore is explored by RawRecordOwnership.cfg, against two ingests.      *)
(***************************************************************************)
EXTENDS RawRecordOwnership
=============================================================================
