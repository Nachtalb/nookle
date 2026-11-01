# Backend image: nookle-api (default), nookle-bot and nookle-tagger, plus static ffmpeg >= 8.
FROM rust:1-alpine AS chef
RUN apk add --no-cache musl-dev ca-certificates && cargo install cargo-chef --locked --version 0.1.73
WORKDIR /src

FROM chef AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p nookle-api -p nookle-bot -p nookle-tagger --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p nookle-api -p nookle-bot -p nookle-tagger \
 && mkdir -p /out/data /out/tmp && cp target/release/nookle-api target/release/nookle-bot target/release/nookle-tagger /out/

FROM mwader/static-ffmpeg:9.0.2-1 AS ffmpeg

FROM scratch
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=ffmpeg /ffmpeg /ffprobe /usr/local/bin/
COPY --from=build /out/nookle-api /out/nookle-bot /out/nookle-tagger /usr/local/bin/
COPY --from=build --chown=65534:65534 /out/data /data
COPY --from=build --chown=65534:65534 /out/tmp /tmp
ENV PATH=/usr/local/bin NOOKLE_DATA=/data NOOKLE_BIND=0.0.0.0:8080 TMPDIR=/tmp
USER 65534:65534
EXPOSE 8080
ENTRYPOINT ["nookle-api"]
