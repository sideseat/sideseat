# Architecture diagrams

Five diagrams, each answering a question that is otherwise answered by reading several thousand lines. They
are checked against the code they describe by `the_diagrams_name_things_that_exist` — every node label that
names a Rust item or an asset section must resolve, so a rename breaks the build rather than the diagram.

Where a diagram and prose disagree, the code is the authority and the diagram is a defect; that is what the
test is for.

## 1. Where framework knowledge lives, and who reads it

The question: *if the Rust knows nothing about frameworks, what does, and when is it consulted?*

The answer is one compiled ruleset with two sets of readers, and the split is the reason a fix does or does
not apply to stored data. Ingestion-side readers ran once, when the span arrived; query-side readers run on
every read, so correcting them corrects history.

```mermaid
flowchart TB
    assets["server/rules/*.json<br/>41 assets · 347 rules"]
    compile["domain::rules::compile<br/>one OnceLock ruleset · digest"]
    assets --> compile

    subgraph plans["Typed plans"]
        direction LR
        detect["detect<br/>DetectPlan"]
        messages["messages<br/>MessagePlan"]
        fields["span_fields<br/>SpanFieldPlan"]
        classify["observation_types<br/>span_categories<br/>ClassifyPlan"]
        carriers["carriers<br/>CarrierPlan"]
        blocks["content_blocks<br/>ContentBlockPlan"]
        members["message_members<br/>MemberPlan"]
        roles["event_roles<br/>provider_aliases<br/>span_facts"]
    end
    compile --> plans

    subgraph ingest["Read at ingestion — the answer is persisted"]
        direction TB
        attrs["traces::extract::attributes"]
        msgs["traces::extract::messages"]
    end

    subgraph query["Read at query time — a fix applies to stored spans"]
        direction TB
        carrier["sideml::carrier"]
        content["sideml::content"]
        normalize["sideml::normalize"]
        order["sideml::feed::order_graph"]
    end

    detect --> attrs
    fields --> attrs
    classify --> attrs
    roles --> attrs
    messages --> msgs
    carriers -->|"one fact:<br/>carrier_holds_span_output"| msgs
    carriers --> carrier
    carriers --> order
    blocks --> content
    members --> content
    roles --> normalize

    compile -. "digest joins the key" .-> cache["sideml::feed::cache<br/>reconstruction memo"]
```

Two things the diagram is making explicit because they are easy to get wrong — and the first is one I drew
incorrectly first, which is why the arrow is annotated rather than plain.

`carriers` is read on **both** sides, asymmetrically. The query side reads all eight facts and the ordering
family, so a correction there reaches stored rows. Ingestion reads exactly one, `carrier_holds_span_output`,
to decide which of a span's messages are its own output — and *that* answer is persisted, so correcting it
does **not** reach spans already stored. "A fix applies to history" is therefore true of carrier semantics
generally and false of that one fact, which is the kind of distinction a diagram either records or quietly
misleads about.

And the ruleset **digest** is part of the reconstruction cache key, because that cache is a memo over a pure
function of the rows and the rules are part of the function.

## 2. Ingestion, and where a 200 becomes true

The question: *at which point has the server promised the data is stored?*

```mermaid
flowchart TB
    otlp["OTLP export<br/>HTTP or gRPC"] --> auth["require_auth<br/>verify_project_access"]
    auth --> fence1{"project_accepts_writes?"}
    fence1 -- "no" --> gone["404 Gone"]
    fence1 -- "yes" --> storable["strip_unstorable_spans<br/>settled at the edge"]
    storable --> durable{"topic backend<br/>durable?"}

    durable -- "Redis" --> publish["XADD + WAITAOF<br/>min_replica_acks"]
    publish --> ack200a["200 — entry is fsynced<br/>and replicated"]
    publish --> consume["consumer group"]

    durable -- "in-memory<br/>(default)" --> now["ingest_now<br/>writes inside the request"]

    consume --> batch
    now --> batch

    subgraph batch["run_batch — per request, parallel across cores"]
        direction TB
        s1["1a extract_attributes_batch<br/>SpanData + framework label"]
        s1 --> s2["1b extract_messages_batch<br/>messages · tool defs · tool names"]
        s2 --> s3["2 to_sideml_batch"]
        s3 --> s4["3 enrich_batch<br/>cost · previews"]
        s4 --> s5["4 prepare_batch<br/>file extraction · NormalizedSpan"]
    end

    batch --> fence2{"drop_spans_for_deleted_traces"}
    fence2 -- "tombstoned" --> drop["dropped · reported"]
    fence2 -- "live" --> files["files and associations written<br/>pending_writers incremented"]
    files --> rows["analytics rows written"]
    rows --> fence3["collect_spans_written_for_deleted_traces<br/>compensate a deletion in the window"]
    fence3 --> confirm["confirm_associations<br/>durable = true"]
    confirm --> sse["SSE published — only surviving spans"]
    sse --> ack200b["200"]
```

The ordering of the two writes is deliberate and is the whole reason the fences look like this: **files
before the rows that name them**, so the surviving failure is a reclaimable orphan rather than a row
promising bytes that are not there.

## 3. How a message rule claims a carrier

The question: *two dialects both describe `output.value` — which one reads it, and why is the answer not
"whichever is first"?*

```mermaid
flowchart TB
    span["span attributes + events"] --> stage{"stage"}
    stage -- "events" --> fromev["MessagePlan::from_event<br/>when_event selects"]
    stage -- "attributes" --> dialect["stage: dialect<br/>rules in rank order"]

    dialect --> gate{"gates_allow?<br/>when / unless / reads_tool_spans"}
    gate -- "no" --> next["next rule — and the raw form<br/>is *not* suppressed"]
    gate -- "yes" --> read["ReadSpec<br/>attribute · attribute_any_of · indexed_family"]

    read --> body{"rule body"}
    body -- "branch_set" --> branch["primary → per-axis emptiness →<br/>fallback_if_primary_empty → always"]
    body -- "compose" --> compose["members joined into one synthetic carrier"]
    body -- "elements / sections / walk" --> traverse["bounded traversal"]
    body -- "plain" --> alts["alternatives (first that yields)<br/>also (all contribute)<br/>fallback (if nothing did)"]

    branch --> emit
    compose --> emit
    traverse --> emit
    alts --> emit

    emit["Emission<br/>rule_id · target · owns: set of carriers"] --> claim{"already claimed<br/>by an earlier rank?"}
    claim -- "yes" --> skip["skipped — one rule per carrier"]
    claim -- "no" --> keep["kept · carriers marked claimed"]

    keep --> fb{"any dialect rule<br/>emitted or claimed?"}
    fb -- "no" --> fallback["stage: fallback<br/>generic raw_io reading"]
```

`owns` is a **set of physical carriers**, not the emitted tag: a rule that composes three attributes into one
observation owns all three, or the other two stay free for another dialect to read as a conversation. A
*claim* emission owns without emitting, which is how a payload that is framework internals rather than a
message is taken off the table.

## 4. Query-time reconstruction

The question: *why can a span view hold more messages than its trace view?*

```mermaid
flowchart TB
    rows["MessageSpanRow<br/>span / trace / session row set"] --> digest["cache key = BLAKE3 of every field read<br/>+ ruleset digest + session_of_trace"]
    digest --> hit{"memo hit?"}
    hit -- "yes" --> answer
    hit -- "no" --> parse

    subgraph pipeline["feed::process_spans"]
        direction TB
        parse["1 parse — raw JSON → SideML"]
        parse --> flatten["2 flatten — one BlockEntry per ContentBlock"]
        flatten --> correlate["3 correlate — id-less results adopt their call's id"]
        correlate --> classify["4 classify — is_output per block"]
        classify --> history["5 mark history — eight phases"]
        history --> dedup["6 dedup — identity + call repeat ordinal"]
        dedup --> withdraw["7 withdraw — clear an unbacked correlated id"]
        withdraw --> sort["8 sort — total order tuple key"]
        sort --> rolefilter["9 role filter — applied to the finished feed"]
    end

    pipeline --> answer["FeedResult<br/>+ replay_matching_complete"]
    answer --> memo["memoised unfiltered"]

    rows -. "session mode only" .-> strip["cross-trace prefix strip<br/>injective match against a partial order"]
    strip -.-> history
```

The span view loads one span and does **no** cross-trace stripping, because that is what the view means:
what this span carried, including the history it re-sent. So it can hold more messages than its trace view,
where dedup collapses the same turn re-sent by every generation span.

## 5. What keeps framework knowledge out of Rust

The question: *the claim is enforced — by what, exactly, and what does each gate not see?*

```mermaid
flowchart LR
    subgraph gates["Enforcement"]
        direction TB
        names["no_production_module_names_a_framework<br/>tokenised · markers derived from asset ids"]
        keys["no_production_module_carries_a_framework_telemetry_key<br/>every dotted string minus our own vocabulary"]
        oracles["17 equivalence oracles<br/>declared vs the code it replaced"]
        goldens["120 committed goldens<br/>4 views · count · order · content · no duplicates"]
        compile2["compile refusals<br/>dead rule · shared rank · unknown result · unreachable declaration"]
    end

    subgraph blind["What none of them sees"]
        direction TB
        b1["a key no asset declares"]
        b2["a value that is not a dotted key"]
        b3["a computed or non-adjacent name"]
        b4["a name inside an ALLCAPS run"]
        b5["prose"]
        b6["source outside server/src"]
        b7["a producer version nobody captured"]
    end

    gates -.->|"limits, stated"| blind
```

The two sweeps are enforcement of a *syntactic* invariant, not semantic proof — which is why the oracles
matter more than the sweeps, and why the blind spots are enumerated rather than implied.
