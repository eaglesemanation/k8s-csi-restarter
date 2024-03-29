FROM docker.io/rustlang/rust:nightly as build
WORKDIR /usr/src

# Install musl-gcc
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools

# Download the target for static linking.
RUN rustup target add x86_64-unknown-linux-musl

# TODO: Include layers for caching, reference: https://github.com/constellation-rs/constellation/blob/27dc89d0d0e34896fd37d638692e7dfe60a904fc/Dockerfile
# Or maybe use cargo-chef: https://github.com/LukeMathWalker/cargo-chef
COPY . ./
RUN cargo build --target x86_64-unknown-linux-musl --release

# Copy the statically-linked binary into a scratch container.
FROM scratch
LABEL org.opencontainers.image.authors="Vladimir Romashchenko <eaglesemanation@gmail.com>"
LABEL org.opencontainers.image.source="https://github.com/eaglesemanation/k8s-csi-restarter"
LABEL org.opencontainers.image.licenses="MIT"

COPY --from=build /usr/src/target/x86_64-unknown-linux-musl/release/k8s-csi-restarter .
USER 1000
ENTRYPOINT ["./k8s-csi-restarter"]
