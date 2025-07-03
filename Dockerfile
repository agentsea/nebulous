# Build stage
FROM rust:1.86-slim-bullseye AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    curl \
    build-essential \
    protobuf-compiler \
    unzip \
    g++ \
    cmake \
    zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# Install sccache using cargo
RUN cargo install sccache

# Set up sccache for Rust
ENV RUSTC_WRAPPER=sccache

# Create a new empty shell project with only Cargo files
WORKDIR /usr/src/nebulous

COPY Cargo.toml Cargo.lock* ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn lib() {}" > src/lib.rs

RUN cargo build --release

# Remove the dummy files and copy actual source code
RUN rm -rf src
COPY . .

# Build with release profile (this will reuse the cached dependencies)
RUN cargo build --release

# Tools stage - install runtime tools
FROM debian:bullseye-slim AS tools

# Install runtime dependencies and tools in a single layer
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    unzip \
    openssh-client \
    gnupg \
    && rm -rf /var/lib/apt/lists/*

# Install rclone, AWS CLI, and Tailscale in parallel
RUN curl -fsSL https://rclone.org/install.sh | bash && \
    curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "awscliv2.zip" && \
    unzip awscliv2.zip && \
    ./aws/install && \
    rm -rf awscliv2.zip aws && \
    curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.noarmor.gpg | tee /usr/share/keyrings/tailscale-archive-keyring.gpg >/dev/null && \
    curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.tailscale-keyring.list | tee /etc/apt/sources.list.d/tailscale.list && \
    apt-get update && apt-get install -y tailscale && \
    rm -rf /var/lib/apt/lists/*

# Runtime stage
FROM debian:bullseye-slim

# Copy tools from tools stage
COPY --from=tools /usr/bin/rclone /usr/bin/rclone
COPY --from=tools /usr/local/bin/aws /usr/local/bin/aws

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

# Copy the binary from builder
COPY --from=builder /usr/src/nebulous/target/release/nebulous /usr/local/bin/nebulous

# Create a symlink for the 'nebu' command to point to 'nebulous'
RUN ln -s /usr/local/bin/nebulous /usr/local/bin/nebu

# Create directory for SQLite database
RUN mkdir -p /data
WORKDIR /data

# Set environment variables
ENV RUST_LOG=info

# Expose the default port
EXPOSE 3000

# Run the binary
CMD ["sh", "-c", "tailscaled --state=/data/tailscaled.state & \
    sleep 5 && \
    tailscale up --authkey=$TS_AUTHKEY --login-server=${TS_LOGINSERVER:-'https://login.tailscale.com'} --hostname=nebu && \
    exec nebu serve --host 0.0.0.0 --port 3000"]
