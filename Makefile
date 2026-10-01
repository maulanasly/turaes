.PHONY: help run dev test test-all lint fmt fmt-check build verify clean migrate proxy-check

CARGO ?= cargo

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

run: ## Run turaes locally (config/default.toml)
	$(CARGO) run -- serve

dev: ## Run with AUTH_DISABLED=1 and debug logging
	AUTH_DISABLED=1 RUST_LOG=debug TURAES_LOG=debug $(CARGO) run -- serve

migrate: ## Apply database migrations
	$(CARGO) run -- migrate

lint: ## Clippy, deny warnings
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt: ## Format the workspace
	$(CARGO) fmt --all

fmt-check: ## Check formatting
	$(CARGO) fmt --all -- --check

test: ## Unit + doc tests
	$(CARGO) test --workspace

test-all: ## Unit + integration tests
	$(CARGO) test --workspace --all-targets

build: ## Release build
	$(CARGO) build --workspace --release

proxy-check: ## Type-check with the optional Pingora proxy enabled (Linux)
	$(CARGO) check -p turaes-proxy --features pingora

verify: lint fmt-check test-all ## The gate: lint + fmt + all tests

clean: ## Remove build artifacts
	$(CARGO) clean
