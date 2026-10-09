------------------------- MODULE StagingCorrections -------------------------
(***************************************************************************)
(* StagingRetirement for the corrections of one metric datapoint. The      *)
(* protocol is the one module; this instance has no record of an export's  *)
(* own (OwnRecord = FALSE), as a datapoint keeps only its winning revision, *)
(* so a superseded export settles on the revision received after it alone.  *)
(* RetiredIsAnswered is what that must not cost: a payload retired          *)
(* unwritten is always answered by a later revision that is stored.         *)
(***************************************************************************)
EXTENDS StagingRetirement
=============================================================================
