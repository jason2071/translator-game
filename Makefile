.DEFAULT_GOAL := help

.PHONY: help install dev build tauri-build release installer rust-build test rust-test check

help: ## Show available commands
	@echo "Game Translator commands"
	@echo "  make install       Install JavaScript dependencies"
	@echo "  make dev           Start the Tauri development app"
	@echo "  make build         Type-check and build the frontend"
	@echo "  make tauri-build   Build the release app and installer"
	@echo "  make release       Release binary only, no installers (fastest)"
	@echo "  make installer     Full build + NSIS/MSI installers (bundle: all)"
	@echo "  make rust-build    Build the Rust core"
	@echo "  make test          Run all Rust tests"
	@echo "  make rust-test     Run Rust library tests"
	@echo "  make check         Run frontend build and Rust tests"

install: ## Install JavaScript dependencies
	pnpm install

dev: ## Start the Tauri development app
	pnpm tauri dev

build: ## Type-check and build the frontend
	pnpm build

tauri-build: ## Build the release app and installer
	pnpm tauri build

release: ## Release binary only, no installers (fastest)
	pnpm tauri build --no-bundle
	@echo "Binary: src-tauri/target/release/game-translator.exe"

installer: ## Full build + the NSIS setup .exe (bundle: nsis)
	pnpm tauri build --bundles nsis
	@echo "Installer exe: src-tauri/target/release/bundle/nsis/"

rust-build: ## Build the Rust core
	cargo build --manifest-path src-tauri/Cargo.toml

test: ## Run all Rust tests
	cargo test --manifest-path src-tauri/Cargo.toml

rust-test: ## Run Rust library tests
	cargo test --manifest-path src-tauri/Cargo.toml --lib

check: build test ## Run the frontend and Rust verification gates
