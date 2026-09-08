# vaked-lsp — the all-in-one LSP gateway

default:
    @just --list

build-lsp:
    cd vaked-lsp && cargo build --release

check-lsp:
    cd vaked-lsp && cargo check && cargo test

run-lsp: build-lsp
    ./target/release/vaked-lsp

# Regenerate the Unreal Engine clangd database and link it into the workspace
[no-cd]
sync-ue-lsp UE_PATH PROJECT_PATH:
    @echo "[+] Regenerating the Unreal Engine Clang Database..."
    {{UE_PATH}}/Engine/Build/BatchFiles/RunUBT.sh -mode=GenerateClangDatabase \
        -project="{{PROJECT_PATH}}" YourGameEditor Development Linux -game
    @echo "[+] Linking compile_commands.json to the workspace root..."
    ln -sf {{PROJECT_PATH}}/compile_commands.json ./compile_commands.json
    @echo "[+] LSP database primed for vaked-lsp."
