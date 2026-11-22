#!/usr/bin/env python3
"""Lab runner for `head-tests-on-base-v1` (#1071): the H, B and M runs.

Pre-registration: `docs/benchmarks/head-tests-on-base-preregistration.toml` (locked).

Runs on one lab host with Docker, under the host-wide benchmark lock, one step at a time.
Everything lives under `--root` (default `/data/htob`), mounted at the same path inside
each container so paths in results are host paths:

    clones/<owner>__<name>      full clones of the eight repositories
    work/<slug>/{head,base}     one worktree per side; M is the base worktree after B,
                                with the test-path overlay applied
    cache/                      toolchains, package stores, build caches (shared)
    target/<name>               one Cargo target directory per repository (shared,
                                incremental across commits; runs are sequential)
    results/<slug>/<side>.json  per side: steps, exit codes, parsed cases
    logs/<slug>/<side>/         full output of every step

Subcommands:

- `images`: build the toolchain images (`htob-rust`, `htob-node`, `htob-python`,
  `htob-go`, `htob-java`).
- `prepare`: clone the repositories and fetch every base and head commit.
- `run --side H|B|M`: for each pull request of `population.json` (or `--only`), set up
  the side's tree, plan the adapter's steps, and run them: `deps` with network (dependency
  fetch and toolchain install), `build` and `test` with `--network none`. Writes
  `results/<slug>/<side>.json`. The pre-registration orders the sides: every H result is
  recorded before any B or M run; `run` refuses B or M while an H result is missing.
- `collect --out runs.json`: gathers every result into the committed `runs.json`.
- `--test`: self-checks for the parsers, the overlay, module attribution and the
  runnable-file rules, with no host, Docker or network.

The adapters (`plan_units`, `steps_for`) run each repository's own test command
restricted to the changed test files; what a file maps to is in `runnable_unit`.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import tempfile
import subprocess
import sys
import time
from typing import Any
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import head_tests_on_base as study  # noqa: E402

LOCK = "/run/expanse-bench/expanse-bench.flock"
NETWORK_ATTEMPTS = 3
SIDES = ("H", "B", "M")
UID = f"{os.getuid()}:{os.getgid()}" if hasattr(os, "getuid") else "1000:1000"

# ----------------------------------------------------------------------------------------
# Toolchain images. Built once on the host; the repository's own pinned toolchain
# (rust-toolchain.toml, packageManager, uv.lock, the Gradle wrapper) is installed into
# the shared cache by the `deps` step.

# `deno compile` (its spec and integration tests) embeds the `denort` runtime binary, which
# the test build looks for next to `deno`; it is built where the commit has it.
DENO_BUILD = (
    "bins='--bin deno --bin test_server'; "
    "grep -q 'name = \"denort\"' cli/rt/Cargo.toml 2>/dev/null && bins=\"$bins --bin denort\"; "
    "echo \"htob: building $bins\"; cargo build --locked $bins"
)

IMAGES = {
    "htob-rust": """
FROM rust:1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \\
      protobuf-compiler cmake clang lld libglib2.0-dev pkg-config python3 \\
      libfontconfig1-dev && rm -rf /var/lib/apt/lists/*
RUN chmod -R a+rX /usr/local/cargo /usr/local/rustup
# deno's LSP and npm integration tests spawn `npm install` against the test server's
# local registry; deno's own CI has Node on the runner.
COPY --from=node:22-bookworm /usr/local/bin/node /usr/local/bin/node
COPY --from=node:22-bookworm /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
      && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx && node --version
""",
    "htob-node": """
FROM node:22-bookworm
RUN corepack enable && npx -y playwright@1.56.1 install-deps chromium \\
      && rm -rf /var/lib/apt/lists/* /root/.npm
""",
    "htob-python": """
FROM python:3.12-bookworm
RUN pip install --no-cache-dir uv==0.9.5
""",
    "htob-go": """
FROM golang:1.26-bookworm
""",
    "htob-java": """
FROM eclipse-temurin:25-jdk
RUN apt-get update && apt-get install -y --no-install-recommends git \\
      && rm -rf /var/lib/apt/lists/*
""",
}

REPO_IMAGE = {
    "astral-sh/ruff": "htob-rust",
    "tokio-rs/tokio": "htob-rust",
    "denoland/deno": "htob-rust",
    "BurntSushi/ripgrep": "htob-rust",
    "vitejs/vite": "htob-node",
    "fastapi/fastapi": "htob-python",
    "golang/go": "htob-go",
    "junit-team/junit5": "htob-java",
}

# ----------------------------------------------------------------------------------------
# Parsers. Each returns {case_id: "passed" | "failed" | "skipped"}.

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
LIBTEST_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored|failed|fail)\b")


def parse_libtest(text: str) -> dict[str, str]:
    """`test <name> ... ok|FAILED|ignored` lines: libtest, and the `file_test_runner`
    harnesses deno's unit, spec and integration targets use."""
    status = {"ok": "passed", "FAILED": "failed", "failed": "failed", "fail": "failed", "ignored": "skipped"}
    cases: dict[str, str] = {}
    for raw in text.splitlines():
        m = LIBTEST_LINE.match(ANSI.sub("", raw).strip())
        if m:
            cases[m.group(1)] = status[m.group(2)]
    return cases


def parse_junit(text: str) -> dict[str, str]:
    """JUnit XML (pytest, Gradle, `deno test --junit-path`): `<classname>::<name>`."""
    cases: dict[str, str] = {}
    root = ET.fromstring(text)
    for tc in root.iter("testcase"):
        cid = f"{tc.get('classname', '')}::{tc.get('name', '')}"
        tags = {child.tag for child in tc}
        if tags & {"failure", "error"}:
            cases[cid] = "failed"
        elif "skipped" in tags:
            cases[cid] = "skipped"
        else:
            cases[cid] = "passed"
    return cases


def parse_vitest_json(text: str, tree: str, prefix: str = "") -> tuple[dict[str, str], list[str]]:
    """Vitest's JSON reporter: `<prefix><file>::<full name>`, and the files that failed
    to load (a file result that failed with no test in it)."""
    data = json.loads(text)
    cases: dict[str, str] = {}
    unloaded = []
    status = {"passed": "passed", "failed": "failed"}
    for result in data.get("testResults", []):
        rel = os.path.relpath(result["name"], tree)
        assertions = result.get("assertionResults", [])
        if not assertions and result.get("status") == "failed":
            unloaded.append(rel)
        for a in assertions:
            cases[f"{prefix}{rel}::{a['fullName']}"] = status.get(a["status"], "skipped")
    return cases, unloaded


def parse_go_test_json(text: str) -> tuple[dict[str, str], list[str]]:
    """`go test -json`: `<package>::<test>`, and the packages that failed to build."""
    cases: dict[str, str] = {}
    broken = []
    status = {"pass": "passed", "fail": "failed", "skip": "skipped"}
    for line in text.splitlines():
        if not line.startswith("{"):
            continue
        ev = json.loads(line)
        action = ev.get("Action")
        if ev.get("Test") and action in status:
            cases[f"{ev['Package']}::{ev['Test']}"] = status[action]
        elif not ev.get("Test") and action == "fail" and ev.get("FailedBuild"):
            broken.append(ev.get("Package", ""))
        elif action == "build-fail":
            broken.append(ev.get("ImportPath", ""))
    return cases, broken


# ----------------------------------------------------------------------------------------
# Overlay and attribution.


def overlay(name_status: list[tuple[str, str]]) -> dict[str, list[str]]:
    """The mirror's overlay from `git diff --no-renames --name-status base head`: every
    test-path file the change adds or modifies is written at its head version, every one
    it deletes is deleted (a rename is a delete and an add)."""
    write = sorted(p for s, p in name_status if study.is_test_path(p) and s[0] != "D")
    delete = sorted(p for s, p in name_status if study.is_test_path(p) and s[0] == "D")
    return {"write": write, "delete": delete}


def rust_module(path: str, crate_dir: str) -> str:
    """The module path of a Rust file under its target's root directory:
    `tests/e2e/main.rs` -> ``, `tests/e2e/a/b.rs` -> `a::b`, `cli/tools/test/mod.rs` ->
    `tools::test` (crate dir `cli`)."""
    rel = path[len(crate_dir) + 1 :] if crate_dir else path
    parts = rel[:-3].split("/") if rel.endswith(".rs") else rel.split("/")
    if parts and parts[-1] in ("mod", "main", "lib"):
        parts = parts[:-1]
    return "::".join(parts)


def attribute_rust(case: str, modules: dict[str, str]) -> str | None:
    """The file a Rust test case is defined in: the longest module (of `modules`, module
    path -> file, every module file of the target) that is a prefix of the case's path."""
    segments = case.split("::")[:-1]
    for k in range(len(segments), -1, -1):
        prefix = "::".join(segments[:k])
        if prefix in modules:
            return modules[prefix]
    return None


RUST_PATH_MOD = re.compile(r'#\[path\s*=\s*"([^"]+)"\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;')


def module_files(tree: Path, crate_dir: str) -> dict[str, str]:
    """Module path -> file for every module file of a target rooted at `crate_dir` (a
    directory, or a single-file target's file)."""
    if crate_dir.endswith(".rs"):
        return {"": crate_dir}
    root = tree / crate_dir
    out: dict[str, str] = {}
    declared: dict[str, str] = {}
    for f in sorted(root.rglob("*.rs")):
        rel = f.relative_to(tree).as_posix()
        mod = rust_module(rel, crate_dir)
        out.setdefault(mod, rel)
        # `#[path = "lsp_tests.rs"] mod lsp;` (deno's tests/integration/mod.rs): module
        # `lsp` is that file, resolved from the declaring file's directory. Without this
        # every case of such a module falls back to the root file.
        for target, name in RUST_PATH_MOD.findall(f.read_text(encoding="utf-8", errors="replace")):
            dest = (f.parent / target).resolve().relative_to(tree.resolve()).as_posix()
            declared[f"{mod}::{name}" if mod else name] = dest
    out.update(declared)
    return out


# ----------------------------------------------------------------------------------------
# Runnable units: what a changed test-path file maps to in each repository.


@dataclass(frozen=True)
class Unit:
    kind: str  # cargo-test | cargo-bin | deno-file | deno-spec | vitest | vitest-e2e | pytest | go | gradle
    target: str  # test target, package, spec filter or file
    crate_dir: str = ""  # Rust: the target's root directory, for attribution
    package: str = ""


def _cargo_package(tree: Path, crate: str) -> str:
    text = (tree / crate / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', text, re.M | re.S)
    return m.group(1) if m else ""


def runnable_unit(repo: str, path: str, tree: Path) -> Unit | None:
    """The unit a changed test file runs as, or None when it is not a test file the
    repository's runner runs (fixtures, test data, benches, harness and support files)."""
    name = study.file_name(path)
    if repo in ("astral-sh/ruff", "tokio-rs/tokio", "BurntSushi/ripgrep") or (
        repo == "denoland/deno" and path.startswith("tests/integration/")
    ):
        m = re.match(r"^(?:(.*)/)?tests/(.+\.rs)$", path)
        if not m or "/fixtures/" in path or "/resources/" in path:
            return None
        crate, rest = m.group(1) or "", m.group(2)
        if repo == "denoland/deno":
            if name == "mod.rs":
                return None
            crate = "tests/integration"
            return Unit("cargo-test", "integration", crate, _cargo_package(tree, crate))
        if not (tree / crate / "Cargo.toml").exists():
            return None
        target = rest.split("/")[0].removesuffix(".rs")
        prefix = f"{crate}/tests" if crate else "tests"
        root = f"{prefix}/{target}" if "/" in rest else path
        return Unit("cargo-test", target, root, _cargo_package(tree, crate))
    if repo == "denoland/deno":
        if re.match(r"^tests/(unit|unit_node)/[^/]+_test\.ts$", path):
            return Unit("deno-file", path.split("/")[1], path)
        if path.startswith("tests/specs/"):
            d = Path(path).parent
            while d.as_posix().startswith("tests/specs/"):
                if (tree / d / "__test__.jsonc").exists():
                    return Unit("deno-spec", d.as_posix()[len("tests/specs/") :])
                d = d.parent
            return None
        if path.startswith("cli/tools/test/") and path.endswith(".rs"):
            return Unit("cargo-bin", "lib", "cli", "deno")
        return None
    if repo == "vitejs/vite":
        if not re.search(r"\.(spec|test)\.[cm]?[tj]sx?$", name):
            return None
        if path.startswith("playground/"):
            return Unit("vitest-e2e", path)
        return Unit("vitest", path)
    if repo == "fastapi/fastapi":
        if path.startswith("tests/") and re.match(r"^test_.*\.py$", name):
            return Unit("pytest", path)
        return None
    if repo == "golang/go":
        if path.startswith("src/") and name.endswith("_test.go"):
            return Unit("go", str(Path(path).parent)[len("src/") :])
        return None
    if repo == "junit-team/junit5":
        m = re.match(r"^(.*?)/src/test/java/(.*)\.java$", path)
        if m and re.search(r"Tests?$", m.group(2)):
            return Unit("gradle", m.group(2).replace("/", "."), package=m.group(1))
        return None
    return None


# ----------------------------------------------------------------------------------------
# Steps per side.


@dataclass
class Step:
    name: str  # deps | build | test
    script: str
    network: bool = False
    timeout: int = 7200
    output: str = ""  # file the parser reads ('' = the step's log)
    parser: str = ""  # libtest | junit | junit-dir | vitest | go
    prefix: str = ""
    units: list[Unit] = field(default_factory=list)  # the units whose cases this step reports
    # False for a fetch that also compiles (Gradle): its failure does not end the side, and
    # the offline build step after it decides build-failure.
    must_pass: bool = True


@dataclass
class Plan:
    image: str
    env: dict[str, str] = field(default_factory=dict)
    steps: list[Step] = field(default_factory=list)
    # In-tree build output `run --clean-target` removes once the side is recorded.
    scratch: list[str] = field(default_factory=list)


DENO_FETCH = r"""git submodule update --init --depth 1 tests/util/std
url=$(sed -n 's/^const PREBUILT_URL: &str = "\(.*\)";$/\1/p' tests/util/server/servers/npm_registry.rs)
if [ -n "$url" ] && [ ! -x third_party/prebuilt/linux64/esbuild-x64 ]; then
  mkdir -p third_party/prebuilt/linux64
  curl -fsSL --retry 3 -o third_party/prebuilt/linux64/esbuild-x64 "${url}linux64/esbuild-x64"
  chmod +x third_party/prebuilt/linux64/esbuild-x64
fi
ls tests/util/std | head -3"""


DENO_WRAPPER = r"""
real="$T/debug/deno.real"
mv "$T/debug/deno" "$real"
cat > "$T/debug/deno" <<'EOF'
#!/bin/sh
if [ "$1" = test ] && [ -n "$HTOB_JUNIT_DIR" ]; then
  shift
  exec "$(dirname "$0")/deno.real" test --junit-path="$HTOB_JUNIT_DIR/$$-$(date +%s%N).xml" "$@"
fi
exec "$(dirname "$0")/deno.real" "$@"
EOF
chmod +x "$T/debug/deno"
trap 'mv -f "$real" "$T/debug/deno"' EXIT
"""


def cargo_target_flag(u: Unit) -> str:
    """`--lib` for a crate's library target (deno's `cli/lib.rs` declares `mod tools`),
    else `--bin <name>`."""
    return "--lib" if u.target == "lib" else f"--bin {u.target}"


def steps_for(repo: str, units: list[Unit], tree: Path, out: Path, cache: Path, target: Path) -> Plan:
    """The adapter: the repository's own test command restricted to `units`."""
    image = REPO_IMAGE[repo]
    env = {"HOME": str(cache / "home"), "INSTA_UPDATE": "no", "NO_COLOR": "1"}
    plan = Plan(image, env)
    if image == "htob-rust":
        env.update(
            CARGO_HOME=str(cache / "cargo"),
            RUSTUP_HOME=str(cache / "rustup"),
            CARGO_TARGET_DIR=str(target),
            T=str(target),
            CARGO_TERM_COLOR="never",
            PATH="/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        deno = repo == "denoland/deno"
        if deno:
            # deno's test utilities find the repository as the parent of the nearest
            # ancestor named `target` of the running test binary, so each tree builds into
            # its own `<tree>/target`. One target shared across commits reused a stale
            # proc-macro build (`symbol_exports.json is out of sync`).
            env.update(CARGO_TARGET_DIR=str(tree / "target"), T=str(tree / "target"))
            plan.scratch.append(str(tree / "target"))
        # A repository without rust-toolchain.toml builds with the stable toolchain the
        # cache holds; one with the file installs its pinned toolchain here. A repository
        # that commits no Cargo.lock (tokio) resolves one here, before every --locked step.
        plan.steps.append(
            Step(
                "deps",
                "rustup default stable >/dev/null && rustup show active-toolchain && cargo --version"
                " && { [ -f Cargo.lock ] || cargo generate-lockfile; } && cargo fetch --locked",
                network=True,
            )
        )
        if deno:
            # The binaries the test harnesses spawn; rusty_v8 downloads its static library
            # during the build, so this build has network.
            # The unit tests import `@std/*` from the `tests/util/std` submodule, and the
            # test server downloads a prebuilt esbuild on first use; both are fetched here.
            plan.steps.append(Step("deps", DENO_FETCH, network=True))
            plan.steps.append(Step("deps", DENO_BUILD, network=True, timeout=14400))
        test_targets = sorted({u.target for u in units if u.kind == "cargo-test"})
        pkgs = {u.target: u.package for u in units if u.kind == "cargo-test"}
        bins = [u for u in units if u.kind == "cargo-bin"]
        feats = " --features full" if repo == "tokio-rs/tokio" else ""
        build = [f"cargo test --locked --offline -p {pkgs[t]} --test {t}{feats} --no-run" for t in test_targets]
        build += [f"cargo test --locked --offline -p {u.package} {cargo_target_flag(u)} --no-run" for u in bins]
        if deno:
            for kind in sorted({u.target for u in units if u.kind == "deno-file"}):
                build.append(f"cargo test --locked --offline --workspace --test {kind} --no-run")
            if any(u.kind == "deno-spec" for u in units):
                build.append("cargo test --locked --offline --workspace --test specs --no-run")
        if build:
            plan.steps.append(Step("build", " && ".join(build), timeout=14400))
        for t in test_targets:
            plan.steps.append(
                Step(
                    "test",
                    f"cargo test --locked --offline -p {pkgs[t]} --test {t}{feats}",
                    parser="libtest",
                    units=[u for u in units if u.kind == "cargo-test" and u.target == t],
                )
            )
        for u in bins:
            plan.steps.append(
                Step(
                    "test",
                    f"cargo test --locked --offline -p {u.package} {cargo_target_flag(u)} -- tools::test",
                    parser="libtest",
                    units=[u],
                )
            )
        for kind in sorted({u.target for u in units if u.kind == "deno-file"}):
            files = [u.crate_dir for u in units if u.kind == "deno-file" and u.target == kind]
            filters = " ".join(shlex.quote(Path(f).stem) for f in files)
            jd = out / f"junit-{kind}"
            script = (
                f"rm -rf {jd} && mkdir -p {jd} && export HTOB_JUNIT_DIR={jd}\n"
                + DENO_WRAPPER
                + f"cargo test --locked --offline --workspace --test {kind} -- {filters}"
            )
            unit_list = [u for u in units if u.kind == "deno-file" and u.target == kind]
            plan.steps.append(Step("test", script, output=str(jd), parser="junit-dir", units=unit_list))
        specs = sorted({u.target for u in units if u.kind == "deno-spec"})
        for s in specs:
            plan.steps.append(
                Step(
                    "test",
                    f"cargo test --locked --offline --workspace --test specs -- {shlex.quote(s.replace('/', '::'))}",
                    parser="libtest",
                    units=[u for u in units if u.kind == "deno-spec" and u.target == s],
                )
            )
    elif image == "htob-node":
        env.update(
            COREPACK_HOME=str(cache / "corepack"),
            COREPACK_ENABLE_DOWNLOAD_PROMPT="0",
            PLAYWRIGHT_BROWSERS_PATH=str(cache / "ms-playwright"),
            npm_config_store_dir=str(cache / "pnpm-store"),
        )
        plan.steps.append(Step("deps", "pnpm install --frozen-lockfile", network=True))
        plan.steps.append(Step("build", "pnpm run build"))
        unit_files = [u.target for u in units if u.kind == "vitest"]
        e2e_files = [u.target for u in units if u.kind == "vitest-e2e"]
        if unit_files:
            o = out / "vitest-unit.json"
            plan.steps.append(
                Step(
                    "test",
                    f"pnpm exec vitest run --reporter=json --outputFile={o} {' '.join(map(shlex.quote, unit_files))}",
                    output=str(o),
                    parser="vitest",
                    units=[u for u in units if u.kind == "vitest"],
                )
            )
        for mode, extra in (("serve", ""), ("build", "VITE_TEST_BUILD=1 ")):
            if e2e_files:
                o = out / f"vitest-{mode}.json"
                plan.steps.append(
                    Step(
                        "test",
                        f"{extra}pnpm exec vitest run -c vitest.config.e2e.ts --reporter=json --outputFile={o} "
                        + " ".join(map(shlex.quote, e2e_files)),
                        output=str(o),
                        parser="vitest",
                        prefix=f"{mode}::",
                        units=[u for u in units if u.kind == "vitest-e2e"],
                    )
                )
    elif image == "htob-python":
        env.update(UV_CACHE_DIR=str(cache / "uv"), UV_PROJECT_ENVIRONMENT=str(tree / ".htob-venv"))
        plan.steps.append(Step("deps", "uv sync --locked --all-extras --all-groups", network=True))
        files = " ".join(shlex.quote(u.target) for u in units)
        o = out / "pytest.xml"
        plan.steps.append(
            Step(
                "test",
                f"uv run --locked --offline --no-sync python -m pytest -p no:cacheprovider --junitxml={o} {files}",
                output=str(o),
                parser="junit",
                units=list(units),
            )
        )
    elif image == "htob-go":
        env.update(GOCACHE=str(cache / "gocache"), GOROOT_BOOTSTRAP="/usr/local/go", GOTOOLCHAIN="local")
        plan.steps.append(Step("build", "cd src && ./make.bash"))
        pkgs = " ".join(sorted({u.target for u in units}))
        o = out / "go-test.json"
        plan.steps.append(
            Step(
                "test",
                f"cd src && ../bin/go test -json -count=1 {pkgs} > {o}",
                output=str(o),
                parser="go",
                units=list(units),
            )
        )
    elif image == "htob-java":
        env.update(GRADLE_USER_HOME=str(cache / "gradle"))
        projects = sorted({u.package for u in units})
        # Gradle resolves a project's classpaths only when it builds them, and junit5's
        # isolated-projects mode refuses a resolve-everything init script, so the fetch
        # builds the test classes online. Its failure does not end the side: the offline
        # build after it fails the same way on a compile error and decides build-failure.
        # The test runtime classpath is resolved only when the test task is stored in the
        # configuration cache, so the fetch also dry-runs (`-m`) each test task.
        tc = " ".join(f":{p}:testClasses" for p in projects) + "".join(
            f" :{u.package}:test --tests {shlex.quote(u.target)}" for u in units
        ) + " -m"
        plan.steps.append(Step("deps", f"./gradlew --no-daemon {tc}", network=True, must_pass=False))
        plan.steps.append(
            Step("build", "./gradlew --no-daemon --offline " + " ".join(f":{p}:testClasses" for p in projects))
        )
        for u in units:
            rd = tree / u.package / "build" / "test-results" / "test"
            plan.steps.append(
                Step(
                    "test",
                    f"rm -rf {rd} && ./gradlew --no-daemon --offline :{u.package}:test --tests {shlex.quote(u.target)}",
                    output=str(rd),
                    parser="junit-dir",
                    units=[u],
                )
            )
    return plan


# ----------------------------------------------------------------------------------------
# Host side: clones, worktrees, Docker.


def slug(pr: dict[str, Any]) -> str:
    return f"{pr['repo'].split('/')[1]}-{pr['pr_number']}"


def git(*args: str, cwd: Path | None = None, check: bool = True) -> str:
    out = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and out.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {out.stderr.strip()}")
    return out.stdout


def clone_dir(root: Path, repo: str) -> Path:
    return root / "clones" / repo.replace("/", "__")


def population_rows(path: Path, only: list[str]) -> list[dict[str, Any]]:
    rows = [r for r in json.loads(path.read_text(encoding="utf-8"))["pull_requests"] if r["in_population"]]
    if only:
        rows = [r for r in rows if slug(r) in only or f"{r['repo']}#{r['pr_number']}" in only]
    return rows


def cmd_images(args: argparse.Namespace) -> int:
    for name, dockerfile in IMAGES.items():
        if args.only and name not in args.only:
            continue
        print(f"building {name}", flush=True)
        subprocess.run(["docker", "build", "-t", name, "-"], input=dockerfile, text=True, check=True)
    return 0


def cmd_prepare(args: argparse.Namespace) -> int:
    rows = population_rows(args.population, args.only)
    for repo in sorted({r["repo"] for r in rows}):
        d = clone_dir(args.root, repo)
        if not d.exists():
            d.parent.mkdir(parents=True, exist_ok=True)
            print(f"cloning {repo}", flush=True)
            for attempt in range(3):
                tmp = d.with_suffix(".partial")
                subprocess.run(["rm", "-rf", str(tmp)], check=True)
                rc = subprocess.run(["git", "clone", "--no-checkout", f"https://github.com/{repo}.git", str(tmp)]).returncode
                if rc == 0:
                    tmp.rename(d)
                    break
                time.sleep(30)
            else:
                raise RuntimeError(f"clone {repo} failed")
        for r in rows:
            if r["repo"] != repo:
                continue
            for sha in (r["base_sha"], r["head_sha"]):
                if subprocess.run(["git", "cat-file", "-e", f"{sha}^{{commit}}"], cwd=d).returncode != 0:
                    print(f"fetching {repo} {sha[:12]}", flush=True)
                    git("fetch", "--quiet", "origin", sha, cwd=d)
    return 0


def worktree(root: Path, pr: dict[str, Any], which: str) -> Path:
    sha = pr["head_sha"] if which == "head" else pr["base_sha"]
    d = root / "work" / slug(pr) / which
    clone = clone_dir(root, pr["repo"])
    if not d.exists():
        d.parent.mkdir(parents=True, exist_ok=True)
        git("worktree", "add", "--detach", str(d), sha, cwd=clone)
    return d


def reset_tree(tree: Path, sha: str) -> None:
    """The side's tree at exactly `sha`: tracked files restored, the overlay's added files
    removed; ignored build output (node_modules, the venv) is kept."""
    git("checkout", "--quiet", "--detach", "--force", sha, cwd=tree)
    git("clean", "-fdq", cwd=tree)


def apply_overlay(tree: Path, clone: Path, head: str, ov: dict[str, list[str]]) -> None:
    for p in ov["delete"]:
        (tree / p).unlink(missing_ok=True)
    for p in ov["write"]:
        data = subprocess.run(["git", "show", f"{head}:{p}"], cwd=clone, capture_output=True, check=True).stdout
        dest = tree / p
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(data)


def docker_step(args: argparse.Namespace, plan: Plan, step: Step, tree: Path, log: Path) -> int:
    cmd = ["flock", LOCK, "docker", "run", "--rm", "--user", UID, "-v", f"{args.root}:{args.root}", "-w", str(tree)]
    if not step.network:
        cmd += ["--network", "none"]
    for k, v in plan.env.items():
        cmd += ["-e", f"{k}={v}"]
    cmd += [plan.image, "bash", "-c", "set -o pipefail\n" + step.script]
    log.parent.mkdir(parents=True, exist_ok=True)
    started = time.time()
    with log.open("w", encoding="utf-8") as fh:
        fh.write(f"$ {step.script}\n")
        fh.flush()
        try:
            rc = subprocess.run(cmd, stdout=fh, stderr=subprocess.STDOUT, timeout=step.timeout).returncode
        except subprocess.TimeoutExpired:
            fh.write(f"\nhtob: timeout after {step.timeout}s\n")
            rc = 124
        fh.write(f"\nhtob: exit {rc} after {time.time() - started:.0f}s\n")
    return rc


def parse_step(step: Step, log: Path, tree: Path) -> tuple[dict[str, str], list[str]]:
    if step.parser == "libtest":
        return parse_libtest(log.read_text(encoding="utf-8", errors="replace")), []
    src = Path(step.output)
    if step.parser == "junit-dir":
        cases: dict[str, str] = {}
        for f in sorted(src.glob("*.xml")) if src.is_dir() else []:
            try:
                cases.update(parse_junit(f.read_text(encoding="utf-8", errors="replace")))
            except ET.ParseError:
                pass
        return cases, []
    if not src.exists():
        return {}, [f"no output file {src.name}"]
    text = src.read_text(encoding="utf-8", errors="replace")
    if step.parser == "junit":
        return parse_junit(text), []
    if step.parser == "vitest":
        return parse_vitest_json(text, str(tree), step.prefix)
    if step.parser == "go":
        return parse_go_test_json(text)
    raise ValueError(step.parser)


GO_TEST_FUNC = re.compile(r"^func ((?:Test|Example|Fuzz|Benchmark)\w*)\(", re.M)


def case_file(u: Unit, cid: str, tree: Path, files: list[str]) -> str | None:
    """The changed test file (or spec directory) a case of unit `u` is defined in, or None
    when the case belongs to a file the change did not touch."""
    if u.kind in ("cargo-test", "cargo-bin"):
        modules = module_files(tree, u.crate_dir)
        # A custom harness that names cases by `module_path!()` (deno's integration
        # target) prefixes the crate name, which libtest does not.
        if cid.startswith(u.target + "::") and u.target not in modules:
            cid = cid[len(u.target) + 2 :]
        f = attribute_rust(cid, modules)
        return f if f in files else None
    if u.kind == "deno-spec":
        name = cid.removeprefix("specs::")
        flt = u.target.replace("/", "::")
        return f"tests/specs/{u.target}" if name == flt or name.startswith(flt + "::") else None
    head = cid.split("::", 1)[0]
    if u.kind == "deno-file":
        return u.crate_dir if head.removeprefix("file://").endswith(u.crate_dir) or head.lstrip("./") == u.crate_dir else None
    if u.kind in ("vitest", "vitest-e2e"):
        bare = cid.split("::", 1)[1] if cid.startswith(("serve::", "build::")) else cid
        return u.target if bare.split("::", 1)[0] == u.target else None
    if u.kind == "pytest":
        stem = u.target.removesuffix(".py").replace("/", ".")
        return u.target if head == stem else None
    if u.kind == "go":
        if head != u.target:
            return None
        test = cid.split("::", 1)[1].split("/", 1)[0]
        for f in files:
            if f.startswith(f"src/{u.target}/") and f.endswith("_test.go"):
                if test in GO_TEST_FUNC.findall((tree / f).read_text(encoding="utf-8", errors="replace")):
                    return f
        return None
    if u.kind == "gradle":
        return f"{u.package}/src/test/java/{u.target.replace('.', '/')}.java" if head.split("$")[0] == u.target else None
    return None


def attribute(cases: dict[str, str], units: list[Unit], files: list[str], tree: Path) -> dict[str, dict[str, str]]:
    """Keep the cases defined in the run's changed test files, each with its file."""
    out: dict[str, dict[str, str]] = {}
    for cid, status in cases.items():
        for u in units:
            f = case_file(u, cid, tree, files)
            if f is not None:
                out[cid] = {"status": status, "file": f}
                break
    return out


def cmd_run(args: argparse.Namespace) -> int:
    rows = population_rows(args.population, args.only)
    side = args.side
    results = args.root / "results"
    if side in ("B", "M"):
        every = population_rows(args.population, [])
        missing = [slug(r) for r in every if not (results / slug(r) / "H.json").exists()]
        if missing:
            print(f"refusing {side}: H not recorded for {len(missing)} pull requests: {missing[:5]}")
            return 2
    for pr in rows:
        s = slug(pr)
        dest = results / s / f"{side}.json"
        if dest.exists() and not args.force:
            continue
        clone = clone_dir(args.root, pr["repo"])
        diff = git("diff", "--no-renames", "--name-status", pr["base_sha"], pr["head_sha"], cwd=clone)
        name_status = [tuple(line.split("\t", 1)) for line in diff.splitlines() if line.strip()]
        ov = overlay(name_status)  # type: ignore[arg-type]
        added = {p for st, p in name_status if st[0] == "A"}
        tree = worktree(args.root, pr, "head" if side == "H" else "base")
        reset_tree(tree, pr["head_sha"] if side == "H" else pr["base_sha"])
        if side == "M":
            apply_overlay(tree, clone, pr["head_sha"], ov)
        # The files this side runs: the head versions for H and M, the base versions of the
        # modified files for B.
        files = [p for p in ov["write"] if side != "B" or p not in added]
        units: list[Unit] = []
        for p in files:
            u = runnable_unit(pr["repo"], p, tree)
            if u is not None and u not in units:
                units.append(u)
        out = args.root / "results" / s / f"{side}-out"
        out.mkdir(parents=True, exist_ok=True)
        record: dict[str, Any] = {
            "repo": pr["repo"],
            "pr_number": pr["pr_number"],
            "side": side,
            "sha": pr["head_sha"] if side == "H" else pr["base_sha"],
            "overlay": ov if side == "M" else None,
            "files": files,
            "units": [u.__dict__ for u in units],
            "steps": [],
            "cases": {},
            "unloaded": [],
        }
        print(f"{s} {side}: {len(units)} units", flush=True)
        if units or side == "B":
            plan = steps_for(pr["repo"], units, tree, out, args.root / "cache", args.root / "target" / pr["repo"].split("/")[1])
            raw_count = 0
            failed = None
            for i, step in enumerate(plan.steps):
                if step.name == "test" and not units:
                    continue
                log = args.root / "logs" / s / side / f"{i:02d}-{step.name}.log"
                # A step with network access retries a failed fetch (a download that timed
                # out); a step without network runs once.
                attempts = 0
                while True:
                    attempts += 1
                    rc = docker_step(args, plan, step, tree, log)
                    if rc == 0 or not step.network or attempts == NETWORK_ATTEMPTS:
                        break
                entry = {"name": step.name, "exit": rc, "log": str(log.relative_to(args.root)), "attempts": attempts}
                if step.name == "test":
                    cases, unloaded = parse_step(step, log, tree)
                    raw_count += len(cases)
                    record["cases"].update(attribute(cases, step.units, files, tree))
                    record["unloaded"] += unloaded
                    entry["parsed"] = len(cases)
                record["steps"].append(entry)
                print(f"  {step.name}: exit {rc}", flush=True)
                if rc != 0 and step.name != "test" and step.must_pass:
                    failed = step.name
                    break
            record["failed_step"] = failed
            record["raw_case_count"] = raw_count
            if args.clean_target:
                for d in plan.scratch:
                    shutil.rmtree(d, ignore_errors=True)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return 0


def cmd_collect(args: argparse.Namespace) -> int:
    runs = []
    for pr in population_rows(args.population, []):
        entry: dict[str, Any] = {"repo": pr["repo"], "pr_number": pr["pr_number"]}
        for side in SIDES:
            f = args.root / "results" / slug(pr) / f"{side}.json"
            entry[side] = json.loads(f.read_text(encoding="utf-8")) if f.exists() else None
        runs.append(entry)
    args.out.write_text(json.dumps({"study_id": "head-tests-on-base-v1", "runs": runs}, indent=2) + "\n", encoding="utf-8")
    return 0


# ----------------------------------------------------------------------------------------


def self_test() -> int:
    failures: list[str] = []
    ran: list[str] = []

    def check(name: str, got: Any, want: Any) -> None:
        ran.append(name)
        if got != want:
            failures.append(name)
            print(f"FAIL {name}: {got!r} != {want!r}")

    lib = "running 3 tests\ntest a::b ... ok\ntest c ... FAILED\ntest d ... ignored, slow\n\x1b[32mtest e::f ... ok\x1b[0m\ntest g ... fail (0ms)\n"
    check("libtest", parse_libtest(lib), {"a::b": "passed", "c": "failed", "d": "skipped", "e::f": "passed", "g": "failed"})
    junit = (
        '<testsuites><testsuite name="s"><testcase classname="tests.test_a" name="test_x"/>'
        '<testcase classname="tests.test_a" name="test_y"><failure message="m"/></testcase>'
        '<testcase classname="tests.test_a" name="test_z"><skipped/></testcase>'
        '<testcase classname="k" name="e"><error/></testcase></testsuite></testsuites>'
    )
    check(
        "junit",
        parse_junit(junit),
        {"tests.test_a::test_x": "passed", "tests.test_a::test_y": "failed", "tests.test_a::test_z": "skipped", "k::e": "failed"},
    )
    vt = json.dumps(
        {
            "testResults": [
                {"name": "/w/t/a/__tests__/x.spec.ts", "status": "passed", "assertionResults": [
                    {"fullName": "d x", "status": "passed"}, {"fullName": "d y", "status": "pending"}]},
                {"name": "/w/t/b/__tests__/y.spec.ts", "status": "failed", "assertionResults": []},
            ]
        }
    )
    check(
        "vitest",
        parse_vitest_json(vt, "/w/t", "serve::"),
        ({"serve::a/__tests__/x.spec.ts::d x": "passed", "serve::a/__tests__/x.spec.ts::d y": "skipped"}, ["b/__tests__/y.spec.ts"]),
    )
    go = "\n".join(
        json.dumps(e)
        for e in [
            {"Action": "run", "Package": "hash/adler32", "Test": "TestA"},
            {"Action": "pass", "Package": "hash/adler32", "Test": "TestA"},
            {"Action": "fail", "Package": "hash/adler32", "Test": "TestB"},
            {"Action": "fail", "Package": "hash/crc", "FailedBuild": "hash/crc.test"},
        ]
    )
    check("go", parse_go_test_json(go), ({"hash/adler32::TestA": "passed", "hash/adler32::TestB": "failed"}, ["hash/crc"]))
    ns = [("A", "tests/specs/x/main.ts"), ("M", "tests/specs/x/__test__.jsonc"), ("D", "tests/unit/old_test.ts"),
          ("M", "cli/main.rs"), ("A", "crates/a/resources/test/fixtures/A.py")]
    check(
        "overlay",
        overlay(ns),
        {"write": ["crates/a/resources/test/fixtures/A.py", "tests/specs/x/__test__.jsonc", "tests/specs/x/main.ts"],
         "delete": ["tests/unit/old_test.ts"]},
    )
    check("rust_module main", rust_module("crates/t/tests/e2e/main.rs", "crates/t/tests/e2e"), "")
    check("rust_module file", rust_module("crates/t/tests/e2e/a/b.rs", "crates/t/tests/e2e"), "a::b")
    check("rust_module mod", rust_module("cli/tools/test/mod.rs", "cli"), "tools::test")
    check("rust_module single", rust_module("tokio/tests/tcp_shutdown.rs", "tokio/tests"), "tcp_shutdown")
    mods = {"": "t/main.rs", "implementation": "t/implementation.rs", "a": "t/a/mod.rs", "a::b": "t/a/b.rs"}
    check("attribute leaf", attribute_rust("implementation::goto", mods), "t/implementation.rs")
    check("attribute nested", attribute_rust("a::b::tests::x", mods), "t/a/b.rs")
    check("attribute root", attribute_rust("top", mods), "t/main.rs")
    check("attribute inline mod", attribute_rust("a::inner::x", mods), "t/a/mod.rs")
    tree = Path("/nonexistent")
    for repo, path, kind in [
        ("denoland/deno", "tests/unit/console_test.ts", "deno-file"),
        ("denoland/deno", "tests/unit_node/http_test.ts", "deno-file"),
        ("denoland/deno", "tests/unit/mod.rs", None),
        ("denoland/deno", "tests/testdata/inspector/a.js", None),
        ("denoland/deno", "tests/registry/npm/@denotest/x/1.0.0/index.js", None),
        ("denoland/deno", "ext/web/benches/encoding.rs", None),
        ("denoland/deno", "cli/tools/test/fmt.rs", "cargo-bin"),
        ("denoland/deno", "tests/integration/mod.rs", None),
        ("astral-sh/ruff", "crates/ruff_linter/resources/test/fixtures/airflow/AIR301.py", None),
        ("astral-sh/ruff", "crates/ruff_benchmark/benches/ty_walltime.rs", None),
        ("vitejs/vite", "playground/hmr/__tests__/hmr.spec.ts", "vitest-e2e"),
        ("vitejs/vite", "packages/vite/src/node/__tests__/utils.spec.ts", "vitest"),
        ("vitejs/vite", "playground/test-utils.ts", None),
        ("vitejs/vite", "playground/lib/__tests__/serve.ts", None),
        ("vitejs/vite", "packages/vite/src/node/__tests__/fixtures/config/import-meta/vite.config.ts", None),
        ("fastapi/fastapi", "tests/test_x.py", "pytest"),
        ("golang/go", "src/hash/adler32/adler32_test.go", "go"),
        ("junit-team/junit5", "platform-tests/src/test/java/org/junit/platform/launcher/core/XTests.java", "gradle"),
    ]:
        u = runnable_unit(repo, path, tree)
        check(f"runnable {path}", u.kind if u else None, kind)
    check("go unit", runnable_unit("golang/go", "src/hash/adler32/adler32_test.go", tree), Unit("go", "hash/adler32"))
    dp = steps_for("denoland/deno", [], Path("/r/work/x/head"), Path("/r/o"), Path("/r/c"), Path("/r/target/deno"))
    bp = steps_for("denoland/deno", [Unit("cargo-bin", "lib", "cli", "deno")], Path("/r/w"), Path("/r/o"), Path("/r/c"), Path("/r/t"))
    check("deno lib unit test step", [st.script for st in bp.steps if st.name == "test"], ["cargo test --locked --offline -p deno --lib -- tools::test"])
    jp = steps_for("junit-team/junit5", [Unit("gradle", "org.a.XTests", "", "platform-tests")], Path("/r/w"), Path("/r/o"), Path("/r/c"), Path("/r/t"))
    check("gradle fetch resolves the test runtime", jp.steps[0].script, "./gradlew --no-daemon :platform-tests:testClasses :platform-tests:test --tests org.a.XTests -m")
    check("gradle fetch does not decide", [(st.name, st.must_pass) for st in jp.steps], [("deps", False), ("build", True), ("test", True)])
    check("deno in-tree target", (dp.env["CARGO_TARGET_DIR"], dp.scratch), ("/r/work/x/head/target", ["/r/work/x/head/target"]))
    check("deno builds denort", [st.script for st in dp.steps if st.name == "deps"][2], DENO_BUILD)
    check("denort when present", "--bin denort" in DENO_BUILD and "cli/rt/Cargo.toml" in DENO_BUILD, True)
    check("rust image has npm", "/usr/local/bin/npm" in IMAGES["htob-rust"], True)
    with tempfile.TemporaryDirectory() as td:
        t = Path(td)
        (t / "tests/integration").mkdir(parents=True)
        (t / "tests/integration/mod.rs").write_text('#[path = "lsp_tests.rs"]\nmod lsp;\npub mod other;\n')
        (t / "tests/integration/lsp_tests.rs").write_text("")
        (t / "tests/integration/other.rs").write_text("")
        iu = Unit("cargo-test", "integration", "tests/integration", "integration_tests")
        changed = ["tests/integration/lsp_tests.rs", "tests/integration/mod.rs"]
        check(
            "path-attribute module",
            [case_file(iu, c, t, changed) for c in ("integration::lsp::a", "lsp::a", "integration::other::b", "integration::root_case")],
            ["tests/integration/lsp_tests.rs", "tests/integration/lsp_tests.rs", None, "tests/integration/mod.rs"],
        )
    check("deno fetch step", [st.script for st in dp.steps if st.name == "deps"][1], DENO_FETCH)
    check(
        "gradle unit",
        runnable_unit("junit-team/junit5", "platform-tests/src/test/java/org/a/XTests.java", tree),
        Unit("gradle", "org.a.XTests", package="platform-tests"),
    )
    print(f"{'FAILED' if failures else 'ok'}: {len(ran)} checks")
    return 1 if failures else 0


def main() -> int:
    if "--test" in sys.argv:
        return self_test()
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--root", type=Path, default=Path("/data/htob"))
    p.add_argument("--population", type=Path, default=study.RESULTS_DIR / "population.json")
    sub = p.add_subparsers(dest="cmd", required=True)
    im = sub.add_parser("images")
    im.add_argument("--only", nargs="*", default=[])
    pr = sub.add_parser("prepare")
    pr.add_argument("--only", nargs="*", default=[])
    rn = sub.add_parser("run")
    rn.add_argument("--side", choices=SIDES, required=True)
    rn.add_argument("--only", nargs="*", default=[])
    rn.add_argument("--force", action="store_true")
    rn.add_argument("--clean-target", action="store_true", help="remove in-tree build output after the side")
    co = sub.add_parser("collect")
    co.add_argument("--out", type=Path, default=study.RESULTS_DIR / "runs.json")
    args = p.parse_args()
    return {"images": cmd_images, "prepare": cmd_prepare, "run": cmd_run, "collect": cmd_collect}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
