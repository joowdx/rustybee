# syntax=docker/dockerfile:1
# Build from the repository root:
#   docker build -f deploy/Dockerfile -t rustybee .
# Multi-arch:
#   docker buildx build -f deploy/Dockerfile --platform linux/amd64,linux/arm64 -t rustybee .

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src/server
COPY server/ ./
# CI passes the release version; local builds report server/Cargo.toml.
ARG RUSTYBEE_VERSION
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/server/target \
    cargo build --release --locked --bin rustybee \
 && cp target/release/rustybee /rustybee
RUN mkdir -p /out/data && chown 10001:10001 /out/data

FROM scratch
COPY --from=build /rustybee /rustybee
COPY --from=build --chown=10001:10001 /out/data /data
ENV RUSTYBEE_DATA_DIR=/data \
    RUSTYBEE_BIND=0.0.0.0:8080 \
    RUST_LOG=info
USER 10001:10001
EXPOSE 8080
VOLUME /data
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/rustybee", "healthcheck"]
ENTRYPOINT ["/rustybee"]
CMD ["serve"]
