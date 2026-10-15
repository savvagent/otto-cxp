# Every base image is pinned by digest (the multi-arch index) so a rebuild
# uses exactly the bytes that were reviewed; the tag each digest was read from
# is in the comment. Bump them deliberately, together with a re-read of each
# digest (`skopeo inspect --format '{{.Digest}}' docker://<image>:<tag>`).

# ---------------------------------------------------------------- console
# The web console (apps/web), built first and separately: it shares nothing
# with the server's toolchain, so a Rust edit does not reinstall packages and
# a console edit does not recompile the workspace.
# node:22-bookworm-slim
FROM node@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 AS console

WORKDIR /web
# pnpm 9, the lockfile's major, exactly (CI pins the same major).
RUN corepack enable && corepack prepare pnpm@9.15.9 --activate

# The workspace's manifests alone, so the install is cached until a
# dependency changes. Only @cxp/web's dependencies are installed.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/
COPY sdk/typescript/package.json sdk/typescript/
COPY packages/react/package.json packages/react/
RUN --mount=type=cache,id=otto-cxp-pnpm,target=/root/.local/share/pnpm/store,sharing=locked \
    pnpm install --frozen-lockfile --filter @cxp/web

COPY apps/web apps/web
# The wire fixture the console's type tests read; `tsc -b` checks them too.
COPY crates/cxp-api/tests/fixtures/wire.json crates/cxp-api/tests/fixtures/wire.json
RUN pnpm --filter @cxp/web build

# ---------------------------------------------------------------- build
# rust:1-slim-bookworm
FROM rust@sha256:2c3a22f0a5533ea2dd5a16627bc841228151faa2d4de2644ac9987e4a2f1f2fa AS build

WORKDIR /app
# Only what the server needs. The SDKs and React package are not part of the
# image, and the web console is built in its own stage above, so a change to
# them does not invalidate this layer.
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
# debian:bookworm-slim
FROM debian@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 AS runtime

# Postgres over TLS and every call to the platform verify against these roots.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Nothing here writes to the filesystem or binds a privileged port.
RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin cxp
USER cxp

COPY --from=build /usr/local/bin/cxp-server /usr/local/bin/cxp-server
COPY --from=console /web/apps/web/dist /srv/console

# The console bundle's location is the one path baked into the image, so it
# gets its default here rather than in every deployment.
ENV CXP_BIND=0.0.0.0:8080 \
    CXP_CONSOLE_DIR=/srv/console \
    CXP_LOG_FORMAT=json \
    RUST_LOG=info

EXPOSE 8080

# Exec form: the process is PID 1 and receives SIGTERM directly, so graceful
# shutdown (and the final usage flush) actually runs on every deploy.
ENTRYPOINT ["/usr/local/bin/cxp-server"]
