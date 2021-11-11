FROM rust:1.56.1-alpine
WORKDIR /work

RUN apk add git
RUN git clone --depth 1 https://github.com/metamath/set.mm

COPY ./ ./
RUN cargo build --release

CMD ["sh"]
