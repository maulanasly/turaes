.PHONY: help run dev test test-all lint fmt fmt-check build verify clean migrate proxy-check
.PHONY: ansible-deps ansible-lint ansible-syntax ansible-check ansible-verify ansible-provision ansible-first-app ansible-vault-edit ansible-vault-view

CARGO ?= cargo
ANSIBLE_DIR ?= deploy/ansible

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

js-check: ## Syntax-check frontend ES modules
	sh scripts/check-js.sh

test-js: ## Frontend unit tests (node --test, zero-build)
	node --test tests/frontend/*.test.mjs

build: ## Release build
	$(CARGO) build --workspace --release

proxy-check: ## Type-check with the optional Pingora proxy enabled (Linux)
	$(CARGO) check -p turaes-proxy --features pingora

proxy-build: ## Release build with the Pingora proxy enabled (Linux)
	$(CARGO) build --release --features proxy

verify: lint fmt-check test-all js-check test-js ## The gate: lint + fmt + tests + frontend

clean: ## Remove build artifacts
	$(CARGO) clean

# --- Ansible provisioning (deploy/ansible) --------------------------------

ansible-deps: ## Install ansible-core + collections (controller)
	python3 -m pip install -r $(ANSIBLE_DIR)/requirements.txt
	ansible-galaxy collection install -r $(ANSIBLE_DIR)/requirements.yml

ansible-lint: ## Lint the playbooks/roles
	cd $(ANSIBLE_DIR) && ansible-lint .

ansible-syntax: ## Syntax-check the provision playbook
	cd $(ANSIBLE_DIR) && ansible-playbook --syntax-check playbooks/provision.yml

ansible-check: ## Dry-run against the host (connects, changes nothing)
	cd $(ANSIBLE_DIR) && ansible-playbook --check playbooks/provision.yml

ansible-verify: ansible-lint ansible-syntax ansible-check ## Lint + syntax + dry-run (the gate)

ansible-provision: ## Provision the VPS end-to-end
	cd $(ANSIBLE_DIR) && ansible-playbook playbooks/provision.yml

ansible-first-app: ## Add + deploy the first app only
	cd $(ANSIBLE_DIR) && ansible-playbook playbooks/provision.yml --tags first-app

ansible-vault-edit: ## Edit the encrypted OAuth secrets
	ansible-vault edit $(ANSIBLE_DIR)/group_vars/vault.yml

ansible-vault-view: ## View the encrypted OAuth secrets
	ansible-vault view $(ANSIBLE_DIR)/group_vars/vault.yml
