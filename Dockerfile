FROM rust:1.56.1-alpine
WORKDIR /work

RUN rustup target add wasm32-unknown-unknown
RUN rustup target add wasm32-wasi

# Get wasmer-js
RUN apk add nodejs
RUN apk add npm
RUN npm install -g @wasmer/cli

# Get set.mm repository
RUN apk add git
RUN git clone --depth 1 https://github.com/metamath/set.mm

# Copy the metamath-knife source code into the container
COPY ./ ./

# Do a regular build
#RUN cargo build --release

# Do as WASM build
RUN cargo build --release --target wasm32-wasi
#RUN cargo build --release --target wasm32-unknown-unknown

CMD ["sh"]
