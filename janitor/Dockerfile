# Build the janitor as a static musl binary, then drop it into the official
# registry image. Sharing the base means the janitor can call
# `registry garbage-collect` directly — no Docker socket, no second toolchain.

FROM rust:alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Build offline-friendly: no dependencies to fetch.
RUN cargo build --release --locked
RUN cp target/release/janitor /janitor

FROM registry:2.8.3
COPY --from=build /janitor /usr/local/bin/janitor
# The registry binary lives on PATH in this image; the janitor shells out to it.
ENTRYPOINT ["/usr/local/bin/janitor"]
