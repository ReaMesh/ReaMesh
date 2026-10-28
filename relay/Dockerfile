# ReaMesh relay. Build and run with docker compose (see README.md), or:
#   docker build -t reamesh-relay relay/
#   docker run -d --network host -v reamesh-relay-data:/data reamesh-relay

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked \
    && mkdir /data

# Distroless: the binary needs only glibc and libgcc, and without a shell
# there is little to break into. Runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/reamesh-relay /usr/local/bin/reamesh-relay
COPY --from=build --chown=65532:65532 /data /data
ENV REAMESH_RELAY_DATA=/data
VOLUME /data
EXPOSE 4001/tcp 4001/udp
ENTRYPOINT ["/usr/local/bin/reamesh-relay"]
CMD ["run"]
