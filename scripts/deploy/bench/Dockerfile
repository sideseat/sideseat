# The ingest benchmark image: the release server and the hot-path benchmark, built from this checkout for the
# host's architecture (aarch64 on Apple Silicon and on Ampere A1), so the per-core figures describe the target.
#
#   docker build -t sideseat-bench -f scripts/deploy/bench/Dockerfile .
#   uv run --locked --script scripts/perf/ingest-throughput.py container --cores 1,2,4,8 --memory 2g
#   docker run --rm --cpus 1 --memory 2g sideseat-bench hot-path      # per-stage microseconds per span
#
# The same two commands on a real A1 host confirm the figure there; nothing in the image depends on the
# machine that built it.

FROM rust:1.95-bookworm AS build
# librdkafka is built from source (`cmake-build`).
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# The UI bundle is embedded at compile time and is not what this image measures, and the build context
# leaves the web app out, so a one-line placeholder page stands in for the bundle.
RUN mkdir -p web/dist && cd web/dist && echo '<!doctype html>' > index.html
# The `profiling` profile: release optimisation with thin LTO. Fat LTO over this workspace needs more memory than
# a Colima VM or a 2 GB A1 instance has, and thin LTO measures within a few percent of it.
ENV CARGO_BUILD_JOBS=4
RUN --mount=type=cache,target=/cargo-target --mount=type=cache,target=/usr/local/cargo/registry \
    CARGO_TARGET_DIR=/cargo-target cargo build --locked --profile profiling -p sideseat-server && \
    cp /cargo-target/profiling/sideseat /usr/local/bin/sideseat && \
    CARGO_TARGET_DIR=/cargo-target cargo test --locked --profile profiling -p sideseat-server --test ingest_hot_path \
      --no-run --message-format=json 2>/dev/null | \
      sed -n 's/.*"executable":"\([^"]*ingest_hot_path[^"]*\)".*/\1/p' | head -1 | xargs -I{} cp {} /usr/local/bin/ingest-hot-path

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/sideseat /usr/local/bin/sideseat
COPY --from=build /usr/local/bin/ingest-hot-path /usr/local/bin/ingest-hot-path
# The benchmark reads the corpus from the path it was compiled against.
COPY --from=build /src/server/tests/fixtures/messages /src/server/tests/fixtures/messages
COPY scripts/deploy/bench/entrypoint.sh /usr/local/bin/entrypoint.sh
ENV SIDESEAT_HOST=0.0.0.0 SIDESEAT_PORT=5388 SIDESEAT_DATA_DIR=/data SIDESEAT_SECRETS_BACKEND=file
RUN mkdir -p /data
EXPOSE 5388
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
