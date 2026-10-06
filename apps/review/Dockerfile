FROM rust:1.98-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY public ./public
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN groupadd --gid 10001 review && useradd --uid 10001 --gid review --no-create-home review \
    && mkdir -p /app/data && chown review:review /app/data
WORKDIR /app
COPY --from=build /build/target/release/zeron-review /usr/local/bin/zeron-review
COPY policy.json ./policy.json
USER review
ENV HOST=0.0.0.0 PORT=3080
EXPOSE 3080
CMD ["zeron-review"]
