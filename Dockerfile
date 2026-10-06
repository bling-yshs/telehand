# telehand-server image. Build: docker build -t telehand-server .
FROM rust:1-trixie AS chef
RUN cargo install cargo-chef --locked
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Dependencies only: this layer stays cached until Cargo.toml/Cargo.lock change.
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release -p telehand-server --recipe-path recipe.json
COPY . .
RUN cargo build --release -p telehand-server

FROM debian:trixie-slim
WORKDIR /app
COPY --from=builder /src/target/release/telehand-server /app/telehand-server
ENV PATH="/app:${PATH}" \
    TELEHAND_DATA_DIR=/app/data
VOLUME /app/data
EXPOSE 20250
ENTRYPOINT ["telehand-server"]
CMD ["serve", "--listen", "0.0.0.0:20250"]
