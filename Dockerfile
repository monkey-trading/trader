# The trader bot as one static binary with the console embedded, in an image that holds nothing
# else. Every input is pinned: the base images by digest, the Rust toolchain by the version
# rust-toolchain.toml names (checked below), the musl standard library by SHA-256, the musl C
# compiler from a fixed Debian snapshot, the crates by Cargo.lock, and the console's packages by
# package-lock.json. Each platform builds natively, linux/amd64 and linux/arm64:
#
#   docker buildx build --platform linux/amd64 -t trader .
#   docker buildx build --platform linux/arm64 --target binary --output type=local,dest=out .
#
# VARIANT=production (the default) builds with the `production` feature, as releases are;
# VARIANT=dev builds without it, for the dev scenarios.

ARG VARIANT=production
ARG RUST_VERSION=1.99.1
ARG RUST_STD_MUSL_SHA256_X86_64=1dd241eaeb19ff3b5180461c77bdee93d6cafc669eda9de834e7c06b0ec3ca95
ARG RUST_STD_MUSL_SHA256_AARCH64=2f3eb0d0610e2b7f6b2121d106e2490ac648decfa0c09b30873bdfc60de6c1ab
ARG DEBIAN_SNAPSHOT=20261005T000000Z

# The console's static files, which are the same on every platform.
FROM --platform=$BUILDPLATFORM node:24.22.2-trixie-slim@sha256:54f84b4cf8432b21aaffd5d3ce8d41e12cea0d10d33c09d2f3c9879d49d6b446 AS console
WORKDIR /console
COPY console/package.json console/package-lock.json console/.npmrc ./
RUN npm ci --no-audit --no-fund
COPY console/ ./
RUN npm run build

# The bot, built for the platform's musl target.
FROM rust:${RUST_VERSION}-slim-trixie@sha256:ea0999ff67355d958b8799b6751c879a6d770076d26047dca66f6e51e77930d3 AS build
ARG VARIANT
ARG RUST_VERSION
ARG RUST_STD_MUSL_SHA256_X86_64
ARG RUST_STD_MUSL_SHA256_AARCH64
ARG DEBIAN_SNAPSHOT
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
COPY docker/retry /usr/local/bin/retry
COPY docker/apt.conf /etc/apt/apt.conf.d/80snapshot
# ring compiles C for the target: the musl C compiler, from the snapshot.
RUN printf '%s\n' \
      'Types: deb' \
      "URIs: http://snapshot.debian.org/archive/debian/$DEBIAN_SNAPSHOT" \
      'Suites: trixie trixie-updates' \
      'Components: main' \
      'Signed-By: /usr/share/keyrings/debian-archive-keyring.pgp' \
      '' \
      'Types: deb' \
      "URIs: http://snapshot.debian.org/archive/debian-security/$DEBIAN_SNAPSHOT" \
      'Suites: trixie-security' \
      'Components: main' \
      'Signed-By: /usr/share/keyrings/debian-archive-keyring.pgp' \
      > /etc/apt/sources.list.d/debian.sources \
 && retry apt-get update \
 && retry apt-get install -y --no-install-recommends curl musl-tools xz-utils \
 && rm -rf /var/lib/apt/lists/*
ENV CC_x86_64_unknown_linux_musl=musl-gcc CC_aarch64_unknown_linux_musl=musl-gcc
# The musl standard library for the pinned toolchain, checked against its SHA-256, once the image's
# toolchain is the one rust-toolchain.toml names.
WORKDIR /tmp
COPY rust-toolchain.toml rust-toolchain.toml
RUN arch="$(uname -m)" \
 && case "$arch" in \
      x86_64) sum="$RUST_STD_MUSL_SHA256_X86_64" ;; \
      aarch64) sum="$RUST_STD_MUSL_SHA256_AARCH64" ;; \
      *) echo "no musl standard library pinned for $arch" >&2; exit 1 ;; \
    esac \
 && grep -qx "channel = \"$RUST_VERSION\"" rust-toolchain.toml \
 && rustc --version | grep -q "^rustc $RUST_VERSION " \
 && retry curl -fsSL --connect-timeout 30 --speed-limit 1000 --speed-time 60 -o rust-std.tar.xz \
      "https://static.rust-lang.org/dist/rust-std-$RUST_VERSION-$arch-unknown-linux-musl.tar.xz" \
 && echo "$sum  rust-std.tar.xz" | sha256sum -c - \
 && tar -xJf rust-std.tar.xz \
 && mv "rust-std-$RUST_VERSION-$arch-unknown-linux-musl/rust-std-$arch-unknown-linux-musl/lib/rustlib/$arch-unknown-linux-musl" \
      "$(rustc --print sysroot)/lib/rustlib/" \
 && rm -rf rust-std* rust-toolchain.toml
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=console /console/dist ./console/dist
# The crates download under retry, then the build runs offline.
RUN case "$VARIANT" in \
      production) features=production ;; \
      dev) features= ;; \
      *) echo "VARIANT must be production or dev, not '$VARIANT'" >&2; exit 1 ;; \
    esac \
 && target="$(uname -m)-unknown-linux-musl" \
 && retry cargo fetch --locked \
 && cargo build --frozen --release --target "$target" -p trader ${features:+--features "$features"} \
 && cp "target/$target/release/trader" /trader

# The binary alone, for a release's downloads.
FROM scratch AS binary
COPY --from=build /trader /trader

# The image: the binary on an empty base, as an unprivileged user. Mount the config's directory at
# /config, or name another file with --config.
FROM scratch
LABEL org.opencontainers.image.source="https://github.com/monkey-trading/trader" \
      org.opencontainers.image.licenses="Apache-2.0"
COPY --from=build /trader /trader
USER 65532:65532
ENTRYPOINT ["/trader"]
CMD ["run", "--config", "/config/trader.toml"]
