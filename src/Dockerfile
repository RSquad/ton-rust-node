FROM rust:slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    make \
    clang \
    libssl-dev \
    libzstd-dev \
    libgoogle-perftools-dev

COPY . /node
WORKDIR /node

ARG GIT_BRANCH
ARG GIT_COMMIT
ARG GIT_COMMIT_DATE

ENV GIT_BRANCH=${GIT_BRANCH}
ENV GIT_COMMIT=${GIT_COMMIT}
ENV GIT_COMMIT_DATE=${GIT_COMMIT_DATE}

RUN cargo build --release --bin node

FROM debian:stable-slim

RUN apt-get update && apt-get install -y \
    openssl \
    libzstd1 \
    libgoogle-perftools4 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /node/target/release/node /usr/local/bin/

WORKDIR /node
