# Multi-stage Dockerfile for test_sync (located at node/tests/test_sync/)
# Based on the main TON Node Dockerfile
# Adds Node.js-based watcher server for node management and testing

# ===================================================================
# Stage 1: Build TON Node
# ===================================================================
FROM rust:slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    make \
    clang \
    libssl-dev \
    libzstd-dev \
    libgoogle-perftools-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

# Copy ton-node source (from repository root, 3 levels up)
COPY . /ton-node
WORKDIR /ton-node

# Accept build arguments for git metadata
ARG GIT_BRANCH
ARG GIT_COMMIT
ARG GIT_COMMIT_DATE

# Pass them as environment variables to build.rs
ENV GIT_BRANCH=${GIT_BRANCH}
ENV GIT_COMMIT=${GIT_COMMIT}
ENV GIT_COMMIT_DATE=${GIT_COMMIT_DATE}

# Build TON node binary
RUN cargo build --release --bin node

# ===================================================================
# Stage 2: Runtime with Node.js and Watcher Server
# ===================================================================
FROM debian:stable-slim

# Install runtime dependencies for TON node and Node.js
RUN apt-get update && apt-get install -y \
    openssl \
    libzstd1 \
    libgoogle-perftools4 \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install Node.js (using NodeSource repository for latest LTS)
RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Copy TON node binary from builder stage (переименован в ton-node)
COPY --from=builder /ton-node/target/release/node /usr/local/bin/ton-node

# Create working directory structure
WORKDIR /watcher


# Copy watcher server files
COPY node/tests/test_sync/package.json ./
COPY node/tests/test_sync/server.js ./

# Install Node.js dependencies (form-data for Slack upload)
RUN npm install --production

# Create necessary directories
RUN mkdir -p /main /main/static /db /logs

# Environment variables
ENV NODE_WATCHER_HTTP=0.0.0.0:32080
ENV SERVER_IP=127.0.0.1

# Expose watcher HTTP port
EXPOSE 32080

# Expose TON node ports (if needed)
EXPOSE 9100
EXPOSE 30303

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD curl -f http://localhost:32080/status || exit 1

# Start watcher server (which will manage node and run tests)
CMD ["/usr/bin/node", "server.js"]
