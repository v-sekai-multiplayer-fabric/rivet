# syntax=docker/dockerfile:1
#
# A Rivet actor that serves the engine's WebTransport echo demo.
#
# Build from the rivet repo root, because the runner binary comes from this
# workspace:
#   docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-wt-demo .

# The engine build. The release notes for tag v2026.06.27.1907-multiplayer-fabric
# ship Windows archives only, and they name these Linux images for the server
# side. ENGINE_TAG defaults to that engine tag; fall back to `latest` if the
# registry carries no tag of that name.
ARG ENGINE_IMAGE=ghcr.io/v-sekai-multiplayer-fabric/zone-godot-runtime
ARG ENGINE_TAG=v2026.06.27.1907-multiplayer-fabric

# Build the runner binary from this workspace rather than curling a release, so
# the demo tracks the fork rather than an upstream artifact.
FROM --platform=linux/amd64 rust:1-bookworm AS runner-builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p rivet-container-runner --bin rivet-container-runner \
    && cp target/release/rivet-container-runner /rivet-container-runner

FROM ${ENGINE_IMAGE}:${ENGINE_TAG}

COPY --from=runner-builder /rivet-container-runner /usr/local/bin/rivet-container-runner
COPY container-runner/examples/godot-demo/entrypoint.sh /usr/local/bin/godot-demo-entrypoint

# GODOT_BIN, GODOT_PROJECT and DEMO_SCRIPT let an actor override the engine
# path, the project, and the script without a rebuild. The actor `input` can
# also override them through its `env` map.
ENV GODOT_BIN=godot \
    GODOT_PROJECT=/opt/zone \
    DEMO_SCRIPT=modules/http3/demo/wt_server_demo.gd

# The runner is the entrypoint. Everything after `--` is the child command
# template, which an actor `input.command` may replace.
ENTRYPOINT ["/usr/local/bin/rivet-container-runner", "--", "/usr/local/bin/godot-demo-entrypoint"]
