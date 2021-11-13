FROM rust:1.56.1-alpine
WORKDIR /work

# Get wasmer-js
RUN apk add nodejs
RUN apk add npm
RUN npm install -g @wasmer/cli

# Get set.mm repository
RUN apk add git
RUN git clone --depth 1 https://github.com/metamath/set.mm

ARG TARGET
RUN if [ ${TARGET} ]; then rustup target add ${TARGET}; fi

# Copy the metamath-knife source code into the container
COPY ./ ./

RUN if [ ${TARGET} ]; then cargo build --release --target ${TARGET}; else cargo build --release; fi

CMD ["sh"]
