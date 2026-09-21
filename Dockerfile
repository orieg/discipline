# syntax=docker/dockerfile:1
# Multi-stage Dockerfile for discipline: universal CI/CD sentinel and AI agent diff guard.
# Produces a minimal, non-root, statically linked container image.

# -----------------------------------------------------------------------------
# Stage 1: Builder
# -----------------------------------------------------------------------------
FROM rust:1.90-alpine AS builder

WORKDIR /src

# Install build dependencies for vendored libgit2 (cmake, make, gcc, musl-dev) and git
RUN apk add --no-cache musl-dev build-base cmake git

# Copy manifest, lockfile, docs, and licenses for compilation
COPY Cargo.toml Cargo.lock README.md LICENSE-* ./

# Copy source tree
COPY src/ ./src/

# Compile locked, release-mode static musl binary
RUN cargo build --release --locked

# -----------------------------------------------------------------------------
# Stage 2: Runtime Image
# -----------------------------------------------------------------------------
FROM alpine:3.21@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507

# Install CA certificates for secure checkouts and git for local repository operations
# Configure system-wide safe.directory = '*' while still root before switching to unprivileged user
RUN apk add --no-cache ca-certificates git \
    && addgroup -g 10001 -S discipline \
    && adduser -u 10001 -S -G discipline -h /workspace -s /bin/sh discipline \
    && mkdir -p /workspace \
    && chown -R discipline:discipline /workspace \
    && git config --system --add safe.directory '*'

# Copy statically compiled discipline binary and entrypoint wrapper
COPY --from=builder /src/target/release/discipline /usr/local/bin/discipline
COPY packaging/docker/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/discipline /usr/local/bin/docker-entrypoint.sh

# Use unprivileged non-root user (numeric UID:GID for strict container security policies)
USER 10001:10001
WORKDIR /workspace
VOLUME ["/workspace"]

# Trust mounted workspace directory in ephemeral container sandbox (CVE-2022-24765 relaxed in container)
ENV DISCIPLINE_TRUST_WORKSPACE=1

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["check"]
