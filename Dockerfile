# Use Rust official image for building (1.83+ required for chrono/ICU dependencies)
FROM rust:latest as builder

WORKDIR /app

# Copy the entire repo (needed for context)
COPY . .

# Build the release binary from the rust subdirectory
WORKDIR /app/rust
RUN cargo build --release

# Use a minimal runtime image
FROM debian:bookworm-slim

# Install runtime dependencies (for native-tls)
RUN apt-get update && apt-get install -y \
  ca-certificates \
  libssl3 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/rust/target/release/near-pagerduty-monitor /app/near-pagerduty-monitor

# Run the binary
CMD ["./near-pagerduty-monitor"]

