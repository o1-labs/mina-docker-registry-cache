.PHONY: help build up down logs ps test test-unit test-integration fmt clean gc

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN{FS=":.*?## "}{printf "  %-18s %s\n", $$1, $$2}'

build: ## Build registry + janitor images
	docker compose build

up: ## Start the registry and janitor (detached)
	docker compose up -d

down: ## Stop containers (keep data)
	docker compose down

logs: ## Tail logs
	docker compose logs -f

ps: ## Show service status
	docker compose ps

gc: ## Run a one-off prune + garbage-collect now
	docker compose run --rm -e RUN_ONCE=true janitor

test-unit: ## Run Rust unit tests on the host
	cd janitor && cargo test

test-integration: ## End-to-end: push images, prune, verify (needs Docker)
	./tests/integration.sh

test: test-unit test-integration ## Run all tests

fmt: ## Format the Rust code
	cd janitor && cargo fmt

clean: ## Stop containers and delete the data volume
	docker compose down -v
