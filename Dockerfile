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
COPY --from=builder /src/target/release/telehand-server /usr/local/bin/telehand-server
ENV TELEHAND_DATA_DIR=/data
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["telehand-server"]
CMD ["serve", "--listen", "0.0.0.0:8080"]
