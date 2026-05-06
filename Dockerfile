FROM rust:1.95 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/server ./server
COPY --from=builder /app/target/release/mock_psp ./mock_psp
COPY migrations ./migrations
CMD ["./server"]