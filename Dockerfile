FROM rust:1.88-slim-bullseye AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    build-essential \
    g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/nebulous
COPY Cargo.toml ./

# Pre-build dependencies to cache them
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true
RUN rm -rf src

COPY ./deploy ./deploy
COPY ./src ./src

RUN cargo build --release


FROM debian:bullseye-slim AS binary-only

COPY --from=builder /usr/src/nebulous/target/release/nebulous /usr/local/bin/nebulous

RUN ln -s /usr/local/bin/nebulous /usr/local/bin/nebu


FROM binary-only AS binary-and-tools

# Install runtime dependencies including Tailscale
RUN apt-get update && apt-get install -y \
    ca-certificates \
    openssh-client \
    curl \
    gnupg \
    && curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.noarmor.gpg | tee /usr/share/keyrings/tailscale-archive-keyring.gpg >/dev/null \
    && curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.tailscale-keyring.list | tee /etc/apt/sources.list.d/tailscale.list \
    && apt-get update && apt-get install -y tailscale \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /data
WORKDIR /data
ENV RUST_LOG=info

EXPOSE 3000

CMD ["sh", "-c", "tailscaled --state=/data/tailscaled.state & \
    sleep 5 && \
    tailscale up --authkey=$TS_AUTHKEY --login-server=${TS_LOGINSERVER:-'https://login.tailscale.com'} --hostname=nebu && \
    exec nebu serve --host 0.0.0.0 --port 3000"]
