# syntax=docker/dockerfile:1

# ---------------------------------------------------------------- build
FROM rust:1-slim-bookworm AS build

WORKDIR /app
# Only what the server needs. The SDKs, React package, and web app are not
# part of the image, so a change to them does not invalidate this layer.
# rust-toolchain.toml is left out on purpose: it names `stable` plus
# components, which would make rustup download a second toolchain on every
# uncached build; the base image's stable compiler is the build's toolchain.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Every statement is a runtime `sqlx::query`, never a `query!` macro, so the
# build needs no database. Migrations are embedded by `sqlx::migrate!`.
# The otto-platform crates are git dependencies, so cargo's git checkouts are
# cached alongside the registry. `target/` lives in a cache mount, so the
# binary is copied out in the same RUN.
#
# Cargo decides whether a workspace crate is fresh by comparing source mtimes
# with the artifacts in `target/`. COPY keeps the files' original mtimes, so a
# cached `target/` built later from other sources (another branch) looks newer
# and its stale artifacts get linked. Touching the sources first makes every
# workspace crate rebuild against what was actually copied; dependencies stay
# cached.
RUN --mount=type=cache,id=otto-cxp-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=otto-cxp-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=otto-cxp-target,target=/app/target,sharing=locked \
    find crates -type f -exec touch {} + \
    && cargo build --release --locked -p cxp-server \
    && cp target/release/cxp-server /usr/local/bin/cxp-server

# ---------------------------------------------------------------- runtime
FROM debian:bookworm-slim AS runtime

# Postgres over TLS and every call to the platform verify against these roots.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Nothing here writes to the filesystem or binds a privileged port.
RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin cxp
USER cxp

COPY --from=build /usr/local/bin/cxp-server /usr/local/bin/cxp-server

ENV CXP_BIND=0.0.0.0:8080 \
    CXP_LOG_FORMAT=json \
    RUST_LOG=info

EXPOSE 8080

# Exec form: the process is PID 1 and receives SIGTERM directly, so graceful
# shutdown (and the final usage flush) actually runs on every deploy.
ENTRYPOINT ["/usr/local/bin/cxp-server"]
