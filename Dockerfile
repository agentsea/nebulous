# Build stage
FROM rust:1.88-slim-bullseye AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    curl \
    build-essential \
    protobuf-compiler \
    sqlite3 \
    unzip \
    libsqlite3-dev \
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
# Copy Cargo.toml first and handle missing Cargo.lock
COPY Cargo.toml ./
# Create empty Cargo.lock if it doesn't exist
RUN touch Cargo.lock

# Pre-build dependencies to leverage Docker layer caching
RUN cargo build --release || true

# Now copy actual source code
COPY . .

# Build with release profile
RUN cargo build --release

# Runtime stage
FROM debian:bullseye-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    sqlite3 \
    libsqlite3-0 \
    curl \
    unzip \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder - fix the binary name
COPY --from=builder /usr/src/nebulous/target/release/nebulous /usr/local/bin/nebulous

# Create a symlink for the 'nebu' command to point to 'nebulous'
RUN ln -s /usr/local/bin/nebulous /usr/local/bin/nebu

# Install rclone
RUN curl https://rclone.org/install.sh | bash

# Install AWS CLI
RUN curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "awscliv2.zip" && \
    unzip awscliv2.zip && \
    ./aws/install && \
    rm -rf awscliv2.zip aws

# Install Tailscale
# RUN curl -fsSL https://tailscale.com/install.sh | sh
RUN curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.noarmor.gpg | tee /usr/share/keyrings/tailscale-archive-keyring.gpg >/dev/null
RUN curl -fsSL https://pkgs.tailscale.com/stable/debian/bullseye.tailscale-keyring.list | tee /etc/apt/sources.list.d/tailscale.list
RUN apt-get update && apt-get install -y tailscale

# Create directory for SQLite database
RUN mkdir -p /data
WORKDIR /data

# Set environment variables
ENV RUST_LOG=info
ENV DATABASE_URL=sqlite:/data/nebulous.db

# Expose the default port
EXPOSE 3000

# Run the binary
CMD ["sh", "-c", "tailscaled --state=/data/tailscaled.state & \
    sleep 5 && \
    tailscale up --authkey=$TS_AUTHKEY --login-server=${TS_LOGINSERVER:-'https://login.tailscale.com'} --hostname=nebu && \
    exec nebu serve --host 0.0.0.0 --port 3000"]
