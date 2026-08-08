# syntax=docker/dockerfile:1
#
# A Rivet actor that serves a headless Godot MCP zone.
#
# Build from the rivet repo root, because the runner binary comes from this
# workspace:
#   docker build -f container-runner/examples/godot-demo/Dockerfile -t godot-mcp-zone .

# The fork's build, not upstream. It is compiled with double precision, which
# is a compile-time property: mixing a double-precision build with a
# single-precision one desynchronises rather than failing loudly, so the runtime
# has to be pinned. See RFD 0011 on Godot runtime provenance.
ARG GODOT_REPO=v-sekai-multiplayer-fabric/godot-images
ARG GODOT_RELEASE=v2026.06.27.1907-multiplayer-fabric

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

# The fork's editor build runs headless with `--headless`, so there is no
# separate server download. The archive nests the binary under a directory named
# for the build's commit, so it is located rather than assumed.
FROM --platform=linux/amd64 debian:trixie-slim AS godot-fetch
ARG GODOT_REPO
ARG GODOT_RELEASE
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/godot.zip \
      "https://github.com/${GODOT_REPO}/releases/download/${GODOT_RELEASE}/linux-editor.zip" \
    && unzip -q /tmp/godot.zip -d /tmp/godot \
    && mv "$(find /tmp/godot -type f -name 'godot.linuxbsd.editor.double.*' | head -1)" /godot \
    && chmod +x /godot \
    && rm -rf /tmp/godot /tmp/godot.zip

# Fail here rather than at actor start if the wrong build was published. A
# single-precision runtime is not a substitute: it desynchronises silently.
RUN /godot --headless --version | grep -q '\.double\.'

# trixie, not bookworm: the fork's build requires GLIBC 2.38 and bookworm ships
# 2.36, which fails at exec with a version error rather than at build time.
FROM --platform=linux/amd64 debian:trixie-slim

# Godot links against these even under --headless.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates libfontconfig1 libfreetype6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=godot-fetch /godot /usr/local/bin/godot
COPY --from=runner-builder /rivet-container-runner /usr/local/bin/rivet-container-runner
COPY container-runner/examples/godot-demo/entrypoint.sh /usr/local/bin/godot-demo-entrypoint

# The in-editor MCP addon, which also runs headless. Pinned by commit, because
# the repository publishes no tags.
ARG MCP_REPO=https://github.com/v-sekai-multiplayer-fabric/vsekai-godot-mcp
ARG MCP_COMMIT=580bb5fedc7c1bb56eb38b8377f918d9c5ffc998
ADD ${MCP_REPO}/archive/${MCP_COMMIT}.tar.gz /tmp/mcp.tar.gz
RUN mkdir -p /opt/zone/addons \
    && tar -xzf /tmp/mcp.tar.gz -C /tmp \
    && cp -r /tmp/vsekai-godot-mcp-${MCP_COMMIT}/addons/vsekai_godot_mcp /opt/zone/addons/ \
    && rm -rf /tmp/mcp.tar.gz /tmp/vsekai-godot-mcp-${MCP_COMMIT}

COPY container-runner/examples/godot-demo/project /opt/zone

# Godot writes its config and import cache under $HOME.
ENV HOME=/tmp \
    GODOT_BIN=godot \
    GODOT_PROJECT=/opt/zone \
    ZONE_SCRIPT=zone_main.gd

# Import the project once so the first actor start does not pay for it.
RUN godot --headless --path /opt/zone --import || true

# The runner is the entrypoint. Everything after `--` is the child command
# template, which an actor `input.command` may replace.
# The default TCP readiness probe works, because the child's MCP listener is TCP.
ENTRYPOINT ["/usr/local/bin/rivet-container-runner", "--", "/usr/local/bin/godot-demo-entrypoint"]
