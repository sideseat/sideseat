-------------------------- MODULE RawRecordRestore --------------------------
(***************************************************************************)
(* RawRecordOwnership through a backup and restore of the embedded stores,  *)
(* under a legal hold. Restore exists only where the transactional and      *)
(* analytics stores are files beside each other (`backup-restore.mdx`), so  *)
(* the writes are DuckDB's atomic look-and-write. The protocol is the one   *)
(* module; this is a fourth instance of it, because restore composes with   *)
(* everything and exploring it against every adversary at once does not     *)
(* terminate. The reconciler races are RawRecordReconcilers.cfg's, and the  *)
(* duplicate executions are RawRecordTakeover.cfg's.                        *)
(***************************************************************************)
EXTENDS RawRecordOwnership
=============================================================================
