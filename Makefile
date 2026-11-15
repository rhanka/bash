.PHONY: hooks check-attribution check-no-python check-no-deps build test bench check check-quick commit

ATTRIBUTION_RANGE ?= --all

hooks:
	git config core.hooksPath .githooks

check-attribution:
	sh tools/check-attribution.sh $(ATTRIBUTION_RANGE)

check-no-python:
	@found=$$(git ls-files -- '*.py' '*.pyc' '*.pyi' 'requirements*.txt' 'pyproject.toml' 'Pipfile*' 'setup.cfg'); \
	if [ -n "$$found" ]; then echo "check-no-python: forbidden files:"; echo "$$found"; exit 1; fi; \
	echo "check-no-python: OK"

check-no-deps:
	@node -e 'const fs=require("fs");const {execSync}=require("child_process");const files=execSync("git ls-files -- package.json \"**/package.json\"").toString().split("\n").filter(Boolean);let bad=0;for(const f of files){const p=JSON.parse(fs.readFileSync(f,"utf8"));for(const k of ["dependencies","peerDependencies","optionalDependencies"]){for(const d of Object.keys(p[k]||{})){if(!d.startsWith("@sentropic/")){console.error(f+": "+k+": "+d);bad=1}}}}if(bad)process.exit(1);console.log("check-no-deps: OK ("+files.length+" manifests)")'

build:
	node tools/build.mjs

test: build
	node --test 'test/*.test.mjs'

bench:
	@if [ -f bench/run.mjs ]; then node bench/run.mjs; else echo "bench: no bench/run.mjs yet"; fi

check: check-attribution check-no-python check-no-deps build
	node tools/check.mjs

check-quick: check-attribution check-no-python check-no-deps build
	node tools/check.mjs --quick

commit:
	@test -n "$(MSG)" || { echo 'usage: make commit MSG="type: subject"'; exit 1; }
	git commit -m "$(MSG)"
