# ReaMesh relay. Build and run with docker compose (see README.md), or from
# the repository root (the build also needs relay-proto/ and the logo text):
#   docker build -f relay/Dockerfile -t reamesh-relay .
#   docker run -d --network host -v reamesh-relay-data:/data reamesh-relay

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY relay-proto relay-proto
COPY proprietary/logo/mark.txt proprietary/logo/mark-blocks.txt proprietary/logo/
COPY relay/Cargo.toml relay/Cargo.lock relay/
COPY relay/src relay/src
RUN cd relay && cargo build --release --locked \
    && mkdir /data

# Distroless: the binary needs only glibc and libgcc, and without a shell
# there is little to break into. Runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/relay/target/release/reamesh-relay /usr/local/bin/reamesh-relay
COPY --from=build --chown=65532:65532 /data /data
ENV REAMESH_RELAY_DATA=/data
VOLUME /data
EXPOSE 4001/tcp 4001/udp
ENTRYPOINT ["/usr/local/bin/reamesh-relay"]
CMD ["run"]
