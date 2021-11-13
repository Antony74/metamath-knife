cd ..
docker build --build-arg "TARGET=wasm32-wasi" -t metamath-knife .
docker cp $(docker ps -alq):/work/target/wasm32-wasi/release/metamath-knife.wasm ./wapm_packages/metamath-knife
cp ./README.md ./wapm_packages/metamath-knife
