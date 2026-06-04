# --- Estágio Base Rust: Cache de Dependências ---
FROM rust:1.82-slim-bookworm AS rust-env
ARG TARGETARCH
WORKDIR /build

COPY Cargo.toml ./
COPY api/Cargo.toml api/
COPY engine/Cargo.toml engine/
COPY lb/Cargo.toml lb/

RUN mkdir -p api/src && echo "fn main() {}" > api/src/main.rs && \
    mkdir -p lb/src && echo "fn main() {}" > lb/src/main.rs && \
    mkdir -p engine/src && echo "pub fn dummy() {}" > engine/src/lib.rs && \
    mkdir -p engine/src/bin && echo "fn main() {}" > engine/src/bin/build_index.rs

RUN if [ "$TARGETARCH" = "amd64" ]; then \
    export RUSTFLAGS="-C target-cpu=haswell -C target-feature=+avx2,+fma,+f16c,+bmi2,+popcnt -C link-arg=-s"; \
    else \
    export RUSTFLAGS="-C link-arg=-s"; \
    fi && \
    cargo build --release && \
    rm -rf api/src lb/src engine/src

# --- Estágio Builder Rust ---
FROM rust-env AS rust-builder
ARG TARGETARCH
COPY resources/ ./resources/
COPY api/src ./api/src
COPY engine/src ./engine/src
COPY lb/src ./lb/src

# 1. Gera o dataset.bin primeiro
RUN if [ "$TARGETARCH" = "amd64" ]; then \
    export RUSTFLAGS="-C target-cpu=haswell -C target-feature=+avx2,+fma,+f16c,+bmi2,+popcnt -C link-arg=-s"; \
    else \
    export RUSTFLAGS="-C link-arg=-s"; \
    fi && \
    touch engine/src/bin/build_index.rs && cargo run --release --bin build_index && ls -lh dataset.bin

# 2. Compila a API
RUN if [ "$TARGETARCH" = "amd64" ]; then \
    export RUSTFLAGS="-C target-cpu=haswell -C target-feature=+avx2,+fma,+f16c,+bmi2,+popcnt -C link-arg=-s"; \
    else \
    export RUSTFLAGS="-C link-arg=-s"; \
    fi && \
    touch api/src/main.rs && touch engine/src/lib.rs && cargo build --release --bin rust_api

# --- Estágio Final: Imagem de Produção Enxuta ---
FROM gcr.io/distroless/cc-debian12:latest
WORKDIR /app

COPY --from=rust-builder /build/target/release/rust_api ./rinha-api
COPY --from=rust-builder /build/dataset.bin ./
COPY resources/normalization.json ./resources/
COPY resources/mcc_risk.json ./resources/

CMD ["./rinha-api"]
