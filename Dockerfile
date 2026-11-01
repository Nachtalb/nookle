# Backend image: nookle-api (default), nookle-bot and nookle-tagger on distroless,
# plus a static ffmpeg/ffprobe 9 trimmed to the codecs Nookle reads and writes.
FROM alpine:3.22 AS ffmpeg
RUN apk add --no-cache build-base nasm cmake meson ninja pkgconf curl xz \
    x264-dev libwebp-dev libwebp-static zlib-dev zlib-static
WORKDIR /b
ARG OPUS=1.5.2 DAV1D=1.5.1 SVT=v3.1.2 FFMPEG=9.0.2
RUN curl -fsSL https://downloads.xiph.org/releases/opus/opus-$OPUS.tar.gz | tar xz && cd opus-$OPUS \
 && ./configure --prefix=/usr/local --disable-shared --enable-static --disable-doc --disable-extra-programs \
 && make -j"$(nproc)" install
RUN curl -fsSL https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D/dav1d-$DAV1D.tar.gz | tar xz && cd dav1d-$DAV1D \
 && meson setup build --buildtype release --default-library static --prefix /usr/local --libdir lib \
    -Denable_tools=false -Denable_tests=false \
 && ninja -C build install
# SVT-AV1's AVX-512 code segfaults in this static musl build; the NAS CPU has no AVX-512 anyway
RUN curl -fsSL https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/$SVT/SVT-AV1-$SVT.tar.gz | tar xz && cd SVT-AV1-$SVT \
 && cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF \
    -DCMAKE_INSTALL_PREFIX=/usr/local -DCMAKE_INSTALL_LIBDIR=lib -DENABLE_AVX512=OFF \
 && ninja -C build install
# musl gives threads a tiny default stack; the stack-size flag raises it to 8 MiB.
# ffprobe gets its own build without the encoder libraries, which roughly halves it.
ARG DECODERS=h264,hevc,vp8,vp9,mpeg4,mpeg2video,mpeg1video,h263,prores,theora,mjpeg,png,apng,gif,webp,webp_anim,bmp,tiff,aac,aac_latm,mp3,mp3float,mp2,opus,vorbis,flac,alac,ac3,eac3,pcm_s16le,pcm_s16be,pcm_s24le,pcm_f32le,pcm_u8,wrapped_avframe,rawvideo
RUN curl -fsSL https://ffmpeg.org/releases/ffmpeg-$FFMPEG.tar.xz | tar xJ && cd ffmpeg-$FFMPEG \
 && set -- --pkg-config-flags=--static --extra-ldflags=-static --extra-ldexeflags="-static -Wl,-z,stack-size=8388608" \
    --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug --disable-ffplay \
    --enable-protocol=file,pipe --enable-demuxers --enable-parsers --enable-bsfs --enable-zlib \
 && PKG_CONFIG_PATH=/usr/local/lib/pkgconfig ./configure "$@" --disable-ffprobe --enable-indev=lavfi \
    --enable-gpl --enable-libx264 --enable-libwebp --enable-libsvtav1 --enable-libdav1d --enable-libopus \
    --enable-decoder=$DECODERS,libdav1d \
    --enable-encoder=libx264,libsvtav1,libwebp,libwebp_anim,libopus,aac,png,rawvideo,wrapped_avframe \
    --enable-muxer=mp4,mov,webp,image2,image2pipe,rawvideo,null \
    --enable-filters \
 && make -j"$(nproc)" ffmpeg && strip ffmpeg && mkdir /out && cp ffmpeg /out/ \
 && make distclean && ./configure "$@" --disable-ffmpeg --enable-decoder=$DECODERS \
 && make -j"$(nproc)" ffprobe && strip ffprobe && cp ffprobe /out/

FROM rust:1-alpine AS chef
RUN apk add --no-cache musl-dev && cargo install cargo-chef --locked --version 0.1.73
WORKDIR /src

FROM chef AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p nookle-api -p nookle-bot -p nookle-tagger --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p nookle-api -p nookle-bot -p nookle-tagger \
 && mkdir -p /out/data && cp target/release/nookle-api target/release/nookle-bot target/release/nookle-tagger /out/

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=ffmpeg /out/ffmpeg /out/ffprobe /usr/local/bin/
COPY --from=build /out/nookle-api /out/nookle-bot /out/nookle-tagger /usr/local/bin/
COPY --from=build --chown=65532:65532 /out/data /data
ENV NOOKLE_DATA=/data NOOKLE_BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["nookle-api"]
