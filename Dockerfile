# Musl-based static build using a muslrust builder
# Produces a fully static binary for x86_64-unknown-linux-musl

FROM clux/muslrust:stable as builder
WORKDIR /home/rust/src
COPY . .

# Build static binary for musl target
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
# Copy statically-linked binary
WORKDIR /app
COPY --from=builder /home/rust/src/target/x86_64-unknown-linux-musl/release/gitlab-ci-exporter /gitlab-ci-exporter
# Default config (can be mounted at runtime)
COPY config.toml /app/config.toml

EXPOSE 3000

USER 1000:1000

ENTRYPOINT ["/gitlab-ci-exporter"]
