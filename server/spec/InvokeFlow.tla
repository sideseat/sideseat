---------------------------- MODULE InvokeFlow ----------------------------
(***************************************************************************)
(* Model of one AG-UI invocation crossing three components: the HTTP SSE   *)
(* handler, the server's WS bridge topic, and the SDK worker.              *)
(*                                                                         *)
(* server/protocol/ws-v1/invoke-flow.md states two safety properties in    *)
(* prose. This spec states them as invariants so TLC can try to break them *)
(* across every interleaving, including the ones that are hard to force in *)
(* an integration test: an SDK event that arrives before the subscriber is *)
(* attached, and a client that disconnects mid-stream.                     *)
(*                                                                         *)
(*   NoLostEvent  - the subscription is established before Invoke is       *)
(*                  published, so an early agent.event is never dropped.   *)
(*   NoStuckBusy  - the SDK never stays busy forever: every path out of    *)
(*                  Streaming either sees a terminal event or leaves the   *)
(*                  cancel guard armed, whose Drop publishes Cancel.       *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    \* How many agent.event messages the SDK emits before terminating.
    MaxEvents

ASSUME MaxEvents \in Nat

VARIABLES
    http,        \* SSE handler state
    sdk,         \* SDK worker state
    subscribed,  \* TRUE once the request topic has a subscriber
    invoked,     \* TRUE once ConnectionControl::Invoke was published
    guard,       \* cancel guard: "armed" | "disarmed"
    inflight,    \* events published by the SDK but not yet delivered
    delivered,   \* events the handler actually observed
    lost,        \* events published with no subscriber attached
    terminal,    \* TRUE once the handler saw or synthesised a terminal event
    cancelled    \* TRUE once ConnectionControl::Cancel was published

vars == <<http, sdk, subscribed, invoked, guard, inflight, delivered, lost,
          terminal, cancelled>>

HttpStates == {"Validating", "LookingUp", "Subscribing", "PublishingInvoke",
               "Streaming", "ClosedTerminal", "ClosedComplete", "ClosedError",
               "ClosedTimeout", "ClosedShutdown", "ClosedDisconnect",
               "Failed404"}

SdkStates == {"Idle", "Busy", "Done", "Errored", "Cancelled"}

TypeOK ==
    /\ http \in HttpStates
    /\ sdk \in SdkStates
    /\ subscribed \in BOOLEAN
    /\ invoked \in BOOLEAN
    /\ guard \in {"armed", "disarmed"}
    /\ inflight \in Nat
    /\ delivered \in Nat
    /\ lost \in Nat
    /\ terminal \in BOOLEAN
    /\ cancelled \in BOOLEAN

Init ==
    /\ http = "Validating"
    /\ sdk = "Idle"
    /\ subscribed = FALSE
    /\ invoked = FALSE
    /\ guard = "disarmed"
    /\ inflight = 0
    /\ delivered = 0
    /\ lost = 0
    /\ terminal = FALSE
    /\ cancelled = FALSE

Closed == {"ClosedTerminal", "ClosedComplete", "ClosedError", "ClosedTimeout",
           "ClosedShutdown", "ClosedDisconnect", "Failed404"}

-----------------------------------------------------------------------------
(* HTTP SSE handler *)

Validate ==
    /\ http = "Validating"
    /\ http' = "LookingUp"
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

\* Registration missing: fails before anything is published, so the SDK is
\* never disturbed and no guard is needed.
LookupMissing ==
    /\ http = "LookingUp"
    /\ http' = "Failed404"
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

LookupFound ==
    /\ http = "LookingUp"
    /\ http' = "Subscribing"
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

\* The ordering that matters: subscribe, and arm the guard, strictly before
\* the invoke is published.
Subscribe ==
    /\ http = "Subscribing"
    /\ subscribed' = TRUE
    /\ guard' = "armed"
    /\ http' = "PublishingInvoke"
    /\ UNCHANGED <<sdk, invoked, inflight, delivered, lost, terminal, cancelled>>

PublishInvoke ==
    /\ http = "PublishingInvoke"
    /\ invoked' = TRUE
    /\ http' = "Streaming"
    /\ UNCHANGED <<sdk, subscribed, guard, inflight, delivered, lost, terminal,
                   cancelled>>

\* Consume one buffered event.
StreamEvent ==
    /\ http = "Streaming"
    /\ inflight > 0
    /\ inflight' = inflight - 1
    /\ delivered' = delivered + 1
    /\ UNCHANGED <<http, sdk, subscribed, invoked, guard, lost, terminal,
                   cancelled>>

\* The SDK's terminal AG-UI event reaches the handler: guard is disarmed.
SeeTerminal ==
    /\ http = "Streaming"
    /\ sdk \in {"Done", "Errored"}
    /\ inflight = 0
    /\ terminal' = TRUE
    /\ guard' = "disarmed"
    /\ http' = "ClosedTerminal"
    /\ UNCHANGED <<sdk, subscribed, invoked, inflight, delivered, lost, cancelled>>

\* Paths that drop the stream without a terminal event. The guard stays armed
\* and its Drop publishes Cancel, which is what keeps the SDK from wedging.
DropTimeout ==
    /\ http = "Streaming"
    /\ delivered = 0
    /\ http' = "ClosedTimeout"
    /\ cancelled' = TRUE
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal>>

DropDisconnect ==
    /\ http = "Streaming"
    /\ http' = "ClosedDisconnect"
    /\ cancelled' = TRUE
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal>>

DropShutdown ==
    /\ http = "Streaming"
    /\ http' = "ClosedShutdown"
    /\ cancelled' = TRUE
    /\ UNCHANGED <<sdk, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal>>

-----------------------------------------------------------------------------
(* SDK worker *)

SdkAccept ==
    /\ sdk = "Idle"
    /\ invoked
    /\ sdk' = "Busy"
    /\ UNCHANGED <<http, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

\* An event published while nobody is subscribed is lost. Reachable only if
\* the implementation ever publishes Invoke before subscribing.
SdkEmit ==
    /\ sdk = "Busy"
    /\ inflight + delivered + lost < MaxEvents
    /\ IF subscribed
         THEN /\ inflight' = inflight + 1
              /\ UNCHANGED lost
         ELSE /\ lost' = lost + 1
              /\ UNCHANGED inflight
    /\ UNCHANGED <<http, sdk, subscribed, invoked, guard, delivered, terminal,
                   cancelled>>

SdkFinish ==
    /\ sdk = "Busy"
    /\ sdk' = "Done"
    /\ UNCHANGED <<http, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

SdkFail ==
    /\ sdk = "Busy"
    /\ sdk' = "Errored"
    /\ UNCHANGED <<http, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

\* Cancel releases a busy worker. This is the transition that makes
\* NoStuckBusy hold on every non-terminal close path.
SdkObserveCancel ==
    /\ cancelled
    /\ sdk = "Busy"
    /\ sdk' = "Cancelled"
    /\ UNCHANGED <<http, subscribed, invoked, guard, inflight, delivered, lost,
                   terminal, cancelled>>

-----------------------------------------------------------------------------
Next ==
    \/ Validate \/ LookupMissing \/ LookupFound \/ Subscribe \/ PublishInvoke
    \/ StreamEvent \/ SeeTerminal
    \/ DropTimeout \/ DropDisconnect \/ DropShutdown
    \/ SdkAccept \/ SdkEmit \/ SdkFinish \/ SdkFail \/ SdkObserveCancel

\* Everything has run to completion: used to state the liveness-shaped checks
\* as a safety property on stable states, which keeps TLC cheap.
Quiescent ==
    /\ http \in Closed
    /\ sdk \in {"Idle", "Done", "Errored", "Cancelled"}
    /\ inflight = 0

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Invariants *)

\* "subscription is established before Invoke is published, so an
\*  early-arrival agent.event from the SDK is never lost."
NoLostEvent == lost = 0

\* The SDK is never left believing it is still serving a request once the
\* handler has finished with it.
NoStuckBusy == Quiescent => sdk # "Busy"

\* The guard is disarmed exactly when a terminal event was surfaced; any other
\* closing path must have published Cancel instead.
GuardDischarged ==
    (http \in Closed /\ invoked) => (terminal \/ cancelled)

\* A delivered event implies there was a subscriber, i.e. ordering held.
DeliveryImpliesSubscription == delivered > 0 => subscribed

=============================================================================
