CARGO_MANIFEST ?= rust/e8_mask_codec/Cargo.toml
PROJECT_DATA_DIR ?= $(CURDIR)/data
PROJECT_HOME ?= $(PROJECT_DATA_DIR)/home
CARGO_HOME ?= $(PROJECT_DATA_DIR)/cargo-home
CARGO_TARGET_DIR ?= $(PROJECT_DATA_DIR)/cargo-target
TOOLCHAIN_ROOT ?= $(shell rustc --print sysroot)
TOOLCHAIN_BIN ?= $(TOOLCHAIN_ROOT)/bin
CARGO ?= $(TOOLCHAIN_ROOT)/bin/cargo
RUSTC ?= $(TOOLCHAIN_BIN)/rustc
RUSTDOC ?= $(TOOLCHAIN_BIN)/rustdoc
BIN_DIR ?= $(CARGO_TARGET_DIR)/release
COUNT_BIN ?= $(BIN_DIR)/count_first_n
UI_BIN ?= $(BIN_DIR)/prime_ui
REBUILD_BIN ?= $(BIN_DIR)/rebuild_wheel210_archive
REPAIR_MISSING_BIN ?= $(BIN_DIR)/repair_wheel210_missing
PARALLEL_REPAIR_SCRIPT ?= tools/repair_wheel210_parallel.py
RUN_DIR ?= data/runs/count_1e11
TARGET_N ?= 100000000000
BACKEND ?= hybrid
THREADS ?= 8
SEGMENT_BLOCKS ?= 32768
CHECKPOINT_EVERY ?= 16
CHECKPOINT_INTERVAL_SEC ?= 60
PROGRESS_INTERVAL_SEC ?= 15
CHECKPOINT_FILE ?= $(RUN_DIR)/count.checkpoint
PROGRESS_FILE ?= $(RUN_DIR)/count.progress
PID_FILE ?= $(RUN_DIR)/count.pid
LOG_FILE ?= $(RUN_DIR)/count.log
ARCHIVE_DIR ?= $(RUN_DIR)/archive
ARCHIVE_MODE ?= wheel210
N ?=
UI_RUN_DIR ?= data/ui
UI_HOST ?= 127.0.0.1
UI_PORT ?= 43173
UI_PID_FILE ?= $(UI_RUN_DIR)/ui.pid
UI_LOG_FILE ?= $(UI_RUN_DIR)/ui.log
UI_ARCHIVE_FILE ?= $(UI_RUN_DIR)/archive_dir.txt
WHEEL_REBUILD_LOG ?= $(RUN_DIR)/rebuild_wheel210.log
WHEEL_REBUILD_PID ?= $(RUN_DIR)/rebuild_wheel210.pid
WHEEL_REPAIR_LOG ?= $(RUN_DIR)/repair_wheel210_missing.log
WHEEL_REPAIR_PID ?= $(RUN_DIR)/repair_wheel210_missing.pid
WHEEL_REPAIR_PARALLEL_LOG ?= $(RUN_DIR)/repair_wheel210_parallel.log
WHEEL_REPAIR_PARALLEL_PID ?= $(RUN_DIR)/repair_wheel210_parallel.pid

# ── Docker / Podman ──────────────────────────────────────────────
CONTAINER_RUNTIME ?= docker
IMAGE_NAME        ?= localhost/prime-ui
IMAGE_TAG         ?= latest
CONTAINER_NAME    ?= math_service
CF_TOKEN_FILE     ?= /tmp/CF2.txt
# Extract token: last word on the Authorization: Bearer line
CF_API_TOKEN      ?= $(shell grep -oP '(?<=Bearer )[^\s"]+' $(CF_TOKEN_FILE) 2>/dev/null)
DATA_VOLUME       ?= $(CURDIR)/data
CADDY_VOLUME      ?= caddy_certs

.PHONY: help prime-build test ui-build ui-start ui-status ui-stop count-start count-start-fresh count-stop count-status count-progress count-log count-wheel210-rebuild count-wheel210-status count-wheel210-repair-missing count-wheel210-repair-status count-wheel210-repair-parallel count-wheel210-repair-parallel-status count-restart is-prime next-prime isPrime nextPrime is-prime-big next-prime-big isPrimeBig nextPrimeBig is-prime-big-file next-prime-big-file isPrimeBigFile nextPrimeBigFile docker-build docker-run docker-stop docker-logs docker-status

help:
	@echo "make prime-build"
	@echo "make test"
	@echo "make ui-build"
	@echo "make ui-start [ARCHIVE_DIR=... UI_PORT=4173]"
	@echo "make ui-status"
	@echo "make ui-stop"
	@echo "make count-start [RUN_DIR=... TARGET_N=... BACKEND=cpu|gpu|hybrid THREADS=8 SEGMENT_BLOCKS=4096]"
	@echo "make count-start-fresh"
	@echo "make count-stop"
	@echo "make count-status"
	@echo "make count-progress"
	@echo "make count-log"
	@echo "make count-wheel210-rebuild"
	@echo "make count-wheel210-repair-missing [FROM_CHUNK=21 TO_CHUNK=3048]"
	@echo "make count-wheel210-repair-parallel [FROM_CHUNK=21 TO_CHUNK=3048 WORKERS=8]"
	@echo "make is-prime N=1234567"
	@echo "make next-prime N=1234567"
	@echo "make isPrime N=1234567"
	@echo "make nextPrime N=1234567"
	@echo "make isPrimeBig N=<large_number> [BACKEND=cpu|gpu|hybrid]"
	@echo "make nextPrimeBig N=<large_number> [BACKEND=cpu|gpu|hybrid]"
	@echo "make isPrimeBigFile FILE=<path> [BACKEND=cpu|gpu|hybrid]"
	@echo "make nextPrimeBigFile FILE=<path> [BACKEND=cpu|gpu|hybrid]"
	@echo "make docker-build [IMAGE_NAME=...] [IMAGE_TAG=...]"
	@echo "make docker-run   [CF_API_TOKEN=...] [DATA_VOLUME=...] [CADDY_VOLUME=...]"
	@echo "make docker-stop"
	@echo "make docker-logs"
	@echo "make docker-status"
	@echo "  build/cache env: HOME=$(PROJECT_HOME) CARGO_HOME=$(CARGO_HOME) CARGO_TARGET_DIR=$(CARGO_TARGET_DIR)"

ENV_PREFIX = HOME=$(PROJECT_HOME) PATH=$(TOOLCHAIN_BIN):$$PATH CARGO_HOME=$(CARGO_HOME) CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) RUSTC=$(RUSTC) RUSTDOC=$(RUSTDOC)

prime-build:
	@mkdir -p $(PROJECT_HOME) $(CARGO_HOME) $(CARGO_TARGET_DIR)
	@$(ENV_PREFIX) $(CARGO) build --quiet --release --manifest-path $(CARGO_MANIFEST)

test:
	@mkdir -p $(PROJECT_HOME) $(CARGO_HOME) $(CARGO_TARGET_DIR)
	@$(ENV_PREFIX) $(CARGO) test --quiet --manifest-path $(CARGO_MANIFEST)
	@$(ENV_PREFIX) python3 tools/requirements_smoke_test.py

ui-build: prime-build

ui-start: ui-build
	@mkdir -p $(UI_RUN_DIR)
	@if [ -f $(UI_PID_FILE) ]; then \
		if kill -0 $$(cat $(UI_PID_FILE)) 2>/dev/null; then \
			echo "ui already running pid=$$(cat $(UI_PID_FILE)) url=http://$(UI_HOST):$(UI_PORT)"; \
			exit 0; \
		else \
			rm -f $(UI_PID_FILE); \
		fi; \
	fi
	@printf '%s\n' "$(ARCHIVE_DIR)" > $(UI_ARCHIVE_FILE)
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" $(UI_BIN) --host $(UI_HOST) --port $(UI_PORT) --archive-dir $(ARCHIVE_DIR) --pid-file $(UI_PID_FILE) --log-file $(UI_LOG_FILE) >> $(UI_LOG_FILE) 2>&1'
	@sleep 1
	@if [ -f $(UI_PID_FILE) ] && kill -0 $$(cat $(UI_PID_FILE)) 2>/dev/null; then \
		echo "ui started pid=$$(cat $(UI_PID_FILE)) url=http://$(UI_HOST):$(UI_PORT)"; \
	else \
		echo "ui failed to start; see $(UI_LOG_FILE)"; \
		exit 1; \
	fi

ui-status:
	@archive_dir="$$(cat $(UI_ARCHIVE_FILE) 2>/dev/null || printf '%s' '$(ARCHIVE_DIR)')"; \
	if [ -f $(UI_PID_FILE) ] && kill -0 $$(cat $(UI_PID_FILE)) 2>/dev/null; then \
		echo "ui_status=running"; \
		echo "ui_pid=$$(cat $(UI_PID_FILE))"; \
		echo "ui_url=http://$(UI_HOST):$(UI_PORT)"; \
		echo "ui_archive_dir=$$archive_dir"; \
	else \
		echo "ui_status=stopped"; \
		echo "ui_url=http://$(UI_HOST):$(UI_PORT)"; \
		echo "ui_archive_dir=$$archive_dir"; \
	fi

ui-stop:
	@if [ -f $(UI_PID_FILE) ] && kill -0 $$(cat $(UI_PID_FILE)) 2>/dev/null; then \
		pid=$$(cat $(UI_PID_FILE)); \
		kill -TERM $$pid; \
		rm -f $(UI_PID_FILE); \
		echo "sent SIGTERM to ui pid=$$pid"; \
	elif [ -f $(UI_PID_FILE) ]; then \
		rm -f $(UI_PID_FILE); \
		echo "removed stale ui pid file"; \
	else \
		echo "ui not running"; \
	fi

count-start: prime-build
	@mkdir -p $(RUN_DIR)
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		echo "already running pid=$$(cat $(PID_FILE))"; \
		exit 0; \
	fi
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && echo $$$$ > $(PID_FILE) && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" $(COUNT_BIN) --n $(TARGET_N) --backend $(BACKEND) --threads $(THREADS) --segment-size $(SEGMENT_BLOCKS) --checkpoint $(CHECKPOINT_FILE) --progress $(PROGRESS_FILE) --archive-dir $(ARCHIVE_DIR) --archive-mode $(ARCHIVE_MODE) --checkpoint-every $(CHECKPOINT_EVERY) --checkpoint-interval-sec $(CHECKPOINT_INTERVAL_SEC) --progress-interval-sec $(PROGRESS_INTERVAL_SEC) >> $(LOG_FILE) 2>&1'
	@sleep 1
	@echo "started pid=$$(cat $(PID_FILE))"

count-start-fresh: prime-build
	@mkdir -p $(RUN_DIR)
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		echo "already running pid=$$(cat $(PID_FILE)); stop it first"; \
		exit 1; \
	fi
	@rm -f $(CHECKPOINT_FILE) $(CHECKPOINT_FILE).prev $(PROGRESS_FILE) $(PID_FILE)
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && echo $$$$ > $(PID_FILE) && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" $(COUNT_BIN) --n $(TARGET_N) --backend $(BACKEND) --threads $(THREADS) --segment-size $(SEGMENT_BLOCKS) --checkpoint $(CHECKPOINT_FILE) --progress $(PROGRESS_FILE) --archive-dir $(ARCHIVE_DIR) --archive-mode $(ARCHIVE_MODE) --checkpoint-every $(CHECKPOINT_EVERY) --checkpoint-interval-sec $(CHECKPOINT_INTERVAL_SEC) --progress-interval-sec $(PROGRESS_INTERVAL_SEC) --no-resume >> $(LOG_FILE) 2>&1'
	@sleep 1
	@echo "started fresh pid=$$(cat $(PID_FILE))"

count-stop:
	@if [ -f $(PID_FILE) ] && kill -0 $$(cat $(PID_FILE)) 2>/dev/null; then \
		kill -TERM $$(cat $(PID_FILE)); \
		echo "sent SIGTERM to pid=$$(cat $(PID_FILE))"; \
	else \
		echo "not running"; \
	fi

count-status:
	@python3 tools/count_status.py $(PROGRESS_FILE) $(PID_FILE) $(ARCHIVE_DIR)

count-progress:
	@python3 tools/count_progress_pretty.py $(PROGRESS_FILE) $(PID_FILE) $(ARCHIVE_DIR)

count-log:
	tail -f $(LOG_FILE)

is-prime: prime-build
	@test -n "$(N)" || { echo "usage: make is-prime N=<number>"; exit 2; }
	@$(ENV_PREFIX) $(BIN_DIR)/is_prime --archive-dir $(ARCHIVE_DIR) --n $(N)

next-prime: prime-build
	@test -n "$(N)" || { echo "usage: make next-prime N=<number>"; exit 2; }
	@$(ENV_PREFIX) $(BIN_DIR)/next_prime --archive-dir $(ARCHIVE_DIR) --n $(N)

count-wheel210-rebuild: prime-build
	@mkdir -p $(RUN_DIR)
	@if [ -f $(WHEEL_REBUILD_PID) ] && kill -0 $$(cat $(WHEEL_REBUILD_PID)) 2>/dev/null; then \
		echo "wheel210 rebuild already running pid=$$(cat $(WHEEL_REBUILD_PID))"; \
		exit 0; \
	fi
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && echo $$$$ > $(WHEEL_REBUILD_PID) && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" $(REBUILD_BIN) --archive-dir $(ARCHIVE_DIR) >> $(WHEEL_REBUILD_LOG) 2>&1'
	@sleep 1
	@echo "wheel210 rebuild pid=$$(cat $(WHEEL_REBUILD_PID))"

count-wheel210-status:
	@if [ -f $(WHEEL_REBUILD_PID) ] && kill -0 $$(cat $(WHEEL_REBUILD_PID)) 2>/dev/null; then \
		echo "wheel210 rebuild running pid=$$(cat $(WHEEL_REBUILD_PID))"; \
	else \
		echo "wheel210 rebuild not running"; \
	fi

count-restart: count-stop
	@sleep 2
	@$(MAKE) count-start RUN_DIR=$(RUN_DIR) TARGET_N=$(TARGET_N) BACKEND=$(BACKEND) THREADS=$(THREADS) SEGMENT_BLOCKS=$(SEGMENT_BLOCKS) CHECKPOINT_EVERY=$(CHECKPOINT_EVERY) CHECKPOINT_INTERVAL_SEC=$(CHECKPOINT_INTERVAL_SEC) PROGRESS_INTERVAL_SEC=$(PROGRESS_INTERVAL_SEC)

isPrime: is-prime

nextPrime: next-prime

is-prime-big: prime-build
	@if [ -z "$(N)" ]; then echo "usage: make is-prime-big N=<number>"; exit 1; fi
	@$(ENV_PREFIX) $(BIN_DIR)/is_prime_big --n $(N) --backend $(BACKEND)

next-prime-big: prime-build
	@if [ -z "$(N)" ]; then echo "usage: make next-prime-big N=<number>"; exit 1; fi
	@$(ENV_PREFIX) $(BIN_DIR)/next_prime_big --n $(N) --backend $(BACKEND)

isPrimeBig: is-prime-big

nextPrimeBig: next-prime-big

is-prime-big-file: prime-build
	@if [ -z "$(FILE)" ]; then echo "usage: make is-prime-big-file FILE=<path>"; exit 1; fi
	@if [ ! -f "$(FILE)" ]; then echo "error: file not found: $(FILE)"; exit 1; fi
	@$(ENV_PREFIX) $(BIN_DIR)/is_prime_big --file $(FILE) --backend $(BACKEND)

next-prime-big-file: prime-build
	@if [ -z "$(FILE)" ]; then echo "usage: make next-prime-big-file FILE=<path>"; exit 1; fi
	@if [ ! -f "$(FILE)" ]; then echo "error: file not found: $(FILE)"; exit 1; fi
	@$(ENV_PREFIX) $(BIN_DIR)/next_prime_big --file $(FILE) --backend $(BACKEND)

isPrimeBigFile: is-prime-big-file

nextPrimeBigFile: next-prime-big-file

# ── Docker / Podman targets ──────────────────────────────────────

docker-build:
	@test -n "$(CF_API_TOKEN)" || { echo "ERROR: CF_API_TOKEN is empty — check $(CF_TOKEN_FILE)"; exit 1; }
	$(CONTAINER_RUNTIME) build \
		--network host \
		-f infra/dockerfile \
		-t $(IMAGE_NAME):$(IMAGE_TAG) \
		.

docker-run:
	@test -n "$(CF_API_TOKEN)" || { echo "ERROR: CF_API_TOKEN is empty — check $(CF_TOKEN_FILE)"; exit 1; }
	@if $(CONTAINER_RUNTIME) container exists $(CONTAINER_NAME) 2>/dev/null; then \
		echo "Container '$(CONTAINER_NAME)' already exists — stop it first with: make docker-stop"; \
		exit 1; \
	fi
	$(CONTAINER_RUNTIME) run -d \
		-p 80:80 \
		-p 443:443 \
		-e CF_API_TOKEN="$(CF_API_TOKEN)" \
		-v "$(DATA_VOLUME):/app/data" \
		-v "$(CADDY_VOLUME):/root/.local/share/caddy" \
		--restart unless-stopped \
		--name $(CONTAINER_NAME) \
		$(IMAGE_NAME):$(IMAGE_TAG)
	@echo "Container started: $(CONTAINER_NAME)"
	@echo "Logs: make docker-logs"

docker-stop:
	@$(CONTAINER_RUNTIME) stop $(CONTAINER_NAME) 2>/dev/null && \
		$(CONTAINER_RUNTIME) rm $(CONTAINER_NAME) 2>/dev/null && \
		echo "Container '$(CONTAINER_NAME)' stopped and removed." || \
		echo "Container '$(CONTAINER_NAME)' not running."

docker-logs:
	$(CONTAINER_RUNTIME) logs -f $(CONTAINER_NAME)

docker-status:
	@$(CONTAINER_RUNTIME) ps --filter name=$(CONTAINER_NAME) --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || \
		echo "No container named '$(CONTAINER_NAME)' running."

count-wheel210-repair-missing: prime-build
	@mkdir -p $(RUN_DIR)
	@if [ -f $(WHEEL_REPAIR_PID) ] && kill -0 $$(cat $(WHEEL_REPAIR_PID)) 2>/dev/null; then \
		echo "wheel210 repair already running pid=$$(cat $(WHEEL_REPAIR_PID))"; \
		exit 0; \
	fi
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && echo $$$$ > $(WHEEL_REPAIR_PID) && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" $(REPAIR_MISSING_BIN) --archive-dir $(ARCHIVE_DIR) --from-chunk $${FROM_CHUNK:-21} --to-chunk $${TO_CHUNK:-3048} >> $(WHEEL_REPAIR_LOG) 2>&1'
	@sleep 1
	@echo "wheel210 repair pid=$$(cat $(WHEEL_REPAIR_PID))"

count-wheel210-repair-status:
	@if [ -f $(WHEEL_REPAIR_PID) ] && kill -0 $$(cat $(WHEEL_REPAIR_PID)) 2>/dev/null; then \
		echo "wheel210 repair running pid=$$(cat $(WHEEL_REPAIR_PID))"; \
	else \
		echo "wheel210 repair not running"; \
	fi

count-wheel210-repair-parallel: prime-build
	@mkdir -p $(RUN_DIR)
	@if [ -f $(WHEEL_REPAIR_PARALLEL_PID) ] && kill -0 $$(cat $(WHEEL_REPAIR_PARALLEL_PID)) 2>/dev/null; then \
		echo "wheel210 parallel repair already running pid=$$(cat $(WHEEL_REPAIR_PARALLEL_PID))"; \
		exit 0; \
	fi
	@setsid -f bash -lc 'cd $(CURDIR) && mkdir -p "$(PROJECT_HOME)" "$(CARGO_HOME)" "$(CARGO_TARGET_DIR)" && echo $$$$ > $(WHEEL_REPAIR_PARALLEL_PID) && exec env HOME="$(PROJECT_HOME)" PATH="$(TOOLCHAIN_BIN):$$PATH" CARGO_HOME="$(CARGO_HOME)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" RUSTC="$(RUSTC)" RUSTDOC="$(RUSTDOC)" python3 $(PARALLEL_REPAIR_SCRIPT) --archive-dir $(ARCHIVE_DIR) --from-chunk $${FROM_CHUNK:-21} --to-chunk $${TO_CHUNK:-3048} --workers $${WORKERS:-8} --bin $(REPAIR_MISSING_BIN) >> $(WHEEL_REPAIR_PARALLEL_LOG) 2>&1'
	@sleep 1
	@echo "wheel210 parallel repair pid=$$(cat $(WHEEL_REPAIR_PARALLEL_PID))"

count-wheel210-repair-parallel-status:
	@if [ -f $(WHEEL_REPAIR_PARALLEL_PID) ] && kill -0 $$(cat $(WHEEL_REPAIR_PARALLEL_PID)) 2>/dev/null; then \
		echo "wheel210 parallel repair running pid=$$(cat $(WHEEL_REPAIR_PARALLEL_PID))"; \
	else \
		echo "wheel210 parallel repair not running"; \
	fi
