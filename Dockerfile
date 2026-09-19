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
FROM alpine:3.21

# Install CA certificates for secure checkouts and git for local repository operations
RUN apk add --no-cache ca-certificates git \
    && addgroup -g 10001 -S discipline \
    && adduser -u 10001 -S -G discipline -h /workspace -s /bin/sh discipline \
    && mkdir -p /workspace \
    && chown -R discipline:discipline /workspace

# Copy statically compiled discipline binary from builder stage
COPY --from=builder /src/target/release/discipline /usr/local/bin/discipline

# Use unprivileged non-root user (numeric UID:GID for strict container security policies)
USER 10001:10001
WORKDIR /workspace
VOLUME ["/workspace"]

ENTRYPOINT ["discipline"]
CMD ["check"]
