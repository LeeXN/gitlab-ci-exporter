# syntax=docker/dockerfile:1

FROM clux/muslrust:stable AS builder
WORKDIR /build

# Keep the build context focused on files needed to compile the service.
COPY Cargo.toml ./
COPY Cargo.lock ./
COPY src ./src

RUN cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.22 AS runtime

RUN addgroup -S exporter \
    && adduser -S -G exporter exporter

WORKDIR /app
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/gitlab-ci-exporter /usr/local/bin/gitlab-ci-exporter
COPY config.toml /app/config.toml

# The database is created in the working directory. Keep it writable without
# running the exporter as root, and persist it through the /app volume.
RUN chown -R exporter:exporter /app
VOLUME ["/app"]

EXPOSE 3000

USER exporter

ENTRYPOINT ["/usr/local/bin/gitlab-ci-exporter"]
