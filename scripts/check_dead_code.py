#!/usr/bin/env python3
"""Report library items that neither the binary nor the library itself reaches.

The crate is a library and a binary. An unused `pub` item in a library never
triggers the compiler's `dead_code` lint, because another crate could use it, so
unused code accumulates unseen. This script builds a crate-private copy in a
temporary directory:

  * every `pub mod` and `pub use` of `src/lib.rs` becomes `pub(crate)`;
  * `src/main.rs` is compiled as a module of the library, with `main` as the
    one public entry point, so everything the binary uses counts as used;

and runs `cargo check --lib` on it. Every warning the compiler then reports is an
item nothing reaches (code under `#[cfg(test)]` is not compiled, so an item only
unit tests use is reported too: gate it with `#[cfg(test)]`).

An item kept on purpose, such as library API used by an integration test under
`tests/` or by a fuzz target, is listed in `scripts/dead_code_allow.txt`, one per
line with its reason:

  <file> <item> | <reason>

The script exits 1 when an item is reported that the list does not allow, or
when the list names an item that is no longer reported (a stale entry), and 2
when the build itself fails.

Usage:
  scripts/check_dead_code.py [--target-dir <dir>] [--list]
  scripts/check_dead_code.py --test

  --target-dir  cargo target directory to use (default: inside the temporary
                copy). Pass a cached directory to reuse compiled dependencies.
  --list        print every reported item, allowed or not, and exit 0.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Dict, List, Set, Tuple

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ALLOW_FILE = os.path.join("scripts", "dead_code_allow.txt")
BIN_MODULE = "bin_main"

Item = Tuple[str, str]  # (file relative to the repository root, item name)


def rewrite_lib(text: str) -> str:
    """Make the library's modules and re-exports crate-private and add the binary."""
    text = re.sub(r"(?m)^pub (mod|use) ", r"pub(crate) \1 ", text)
    return (
        text.rstrip("\n")
        + "\n"
        # `main.rs` names the library as `discipline::`; inside the library that
        # path has to resolve to the crate itself.
        + "extern crate self as discipline;\n"
        + f"pub mod {BIN_MODULE};\n"
    )


def rewrite_main(text: str) -> str:
    """Make `main` the public entry point, without moving any line."""
    out, count = re.subn(r"(?m)^fn main\(\)", "pub fn main()", text)
    if count != 1:
        raise SystemExit("check_dead_code: expected exactly one `fn main()` in src/main.rs")
    return out


def load_allow(path: str) -> Dict[Item, str]:
    """Read the allow-list: `<file> <item> | <reason>`, `#` starts a comment line."""
    allowed: Dict[Item, str] = {}
    if not os.path.exists(path):
        return allowed
    with open(path, encoding="utf-8") as handle:
        for number, raw in enumerate(handle, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            head, sep, reason = line.partition("|")
            parts = head.split()
            if not sep or len(parts) != 2 or not reason.strip():
                raise SystemExit(
                    f"check_dead_code: {path}:{number}: expected `<file> <item> | <reason>`"
                )
            allowed[(parts[0], parts[1])] = reason.strip()
    return allowed


def span_text(span: dict) -> str:
    """The source text a single-line span highlights (the item's name)."""
    lines = span.get("text") or []
    if len(lines) != 1:
        return ""
    line = lines[0]
    return line["text"][line["highlight_start"] - 1 : line["highlight_end"] - 1]


def original_path(file_name: str) -> str:
    """Map a path in the copy back to the repository."""
    if file_name == f"src/{BIN_MODULE}.rs":
        return "src/main.rs"
    return file_name


def parse_diagnostics(stream: str) -> List[Tuple[str, int, str, str]]:
    """Every warning of the crate as (file, line, item name, compiler message)."""
    found: List[Tuple[str, int, str, str]] = []
    for raw in stream.splitlines():
        if not raw.startswith("{"):
            continue
        record = json.loads(raw)
        if record.get("reason") != "compiler-message":
            continue
        message = record["message"]
        if message.get("level") != "warning" or not message.get("spans"):
            continue
        text = message["message"]
        for span in message["spans"]:
            if not span.get("is_primary"):
                continue
            name = span_text(span) or text
            found.append((original_path(span["file_name"]), span["line_start"], name, text))
    return sorted(set(found))


def crate_private_build(target_dir: str) -> List[Tuple[str, int, str, str]]:
    """Copy the crate, make it private, run `cargo check --lib`, return its warnings."""
    work = tempfile.mkdtemp(prefix="discipline-dead-code-")
    try:
        shutil.copytree(os.path.join(ROOT, "src"), os.path.join(work, "src"))
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copy(os.path.join(ROOT, name), os.path.join(work, name))
        lib = os.path.join(work, "src", "lib.rs")
        main = os.path.join(work, "src", "main.rs")
        with open(lib, encoding="utf-8") as handle:
            lib_text = handle.read()
        with open(main, encoding="utf-8") as handle:
            main_text = handle.read()
        with open(lib, "w", encoding="utf-8") as handle:
            handle.write(rewrite_lib(lib_text))
        with open(os.path.join(work, "src", f"{BIN_MODULE}.rs"), "w", encoding="utf-8") as handle:
            handle.write(rewrite_main(main_text))
        os.remove(main)
        env = dict(os.environ)
        env["CARGO_TARGET_DIR"] = target_dir or os.path.join(work, "target")
        # A caller's `-D warnings` would stop the build at the first item.
        env.pop("RUSTFLAGS", None)
        done = subprocess.run(
            ["cargo", "check", "--lib", "--locked", "--message-format=json"],
            cwd=work,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        if done.returncode != 0:
            for raw in done.stdout.splitlines():
                if raw.startswith("{"):
                    rendered = (json.loads(raw).get("message") or {}).get("rendered")
                    if rendered:
                        sys.stderr.write(rendered)
            sys.stderr.write(done.stderr)
            raise SystemExit(2)
        return parse_diagnostics(done.stdout)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def self_test() -> None:
    lib = "#![x]\n\npub mod a;\npub mod b;\n\npub use a::T;\npub use b as c;\n"
    rewritten = rewrite_lib(lib)
    assert "pub(crate) mod a;\npub(crate) mod b;" in rewritten, rewritten
    assert "pub(crate) use a::T;\npub(crate) use b as c;" in rewritten, rewritten
    assert rewritten.endswith(f"extern crate self as discipline;\npub mod {BIN_MODULE};\n")
    assert rewritten.splitlines()[:7] == rewrite_lib(lib).splitlines()[:7]
    # The rewrite keeps every original line on its line, so reported lines are true.
    assert [l.replace("(crate)", "") for l in rewritten.splitlines()[:7]] == lib.splitlines()

    main = "use x;\nfn main() -> ExitCode {\n}\nfn other() {}\n"
    assert rewrite_main(main) == "use x;\npub fn main() -> ExitCode {\n}\nfn other() {}\n"
    try:
        rewrite_main("fn not_main() {}\n")
    except SystemExit:
        pass
    else:
        raise AssertionError("a main.rs without `fn main()` must be refused")

    def diagnostic(level: str, file_name: str, line: int, name: str, text: str) -> str:
        source = f"    pub fn {name}() {{}}"
        start = source.index(name) + 1
        span = {
            "is_primary": True,
            "file_name": file_name,
            "line_start": line,
            "text": [{"text": source, "highlight_start": start, "highlight_end": start + len(name)}],
        }
        return json.dumps(
            {"reason": "compiler-message", "message": {"level": level, "message": text, "spans": [span]}}
        )

    stream = "\n".join(
        [
            diagnostic("warning", "src/a.rs", 7, "gone", "function `gone` is never used"),
            diagnostic("warning", f"src/{BIN_MODULE}.rs", 3, "helper", "function `helper` is never used"),
            diagnostic("error", "src/a.rs", 9, "broken", "mismatched types"),
            json.dumps({"reason": "build-finished", "success": True}),
            "not json",
        ]
    )
    assert parse_diagnostics(stream) == [
        ("src/a.rs", 7, "gone", "function `gone` is never used"),
        ("src/main.rs", 3, "helper", "function `helper` is never used"),
    ], parse_diagnostics(stream)

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "allow.txt")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("# comment\n\nsrc/a.rs gone | used by tests/test_a.rs\n")
        assert load_allow(path) == {("src/a.rs", "gone"): "used by tests/test_a.rs"}
        for bad in ("src/a.rs gone\n", "src/a.rs gone |\n", "src/a.rs | reason\n"):
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(bad)
            try:
                load_allow(path)
            except SystemExit:
                continue
            raise AssertionError(f"malformed allow-list line accepted: {bad!r}")
    print("check_dead_code self-test: OK")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target-dir", default="", help="cargo target directory to use")
    parser.add_argument("--list", action="store_true", help="print every reported item and exit 0")
    parser.add_argument("--test", action="store_true", help="run the self-test")
    args = parser.parse_args()
    if args.test:
        self_test()
        return

    allowed = load_allow(os.path.join(ROOT, ALLOW_FILE))
    target_dir = os.path.abspath(args.target_dir) if args.target_dir else ""
    reported = crate_private_build(target_dir)
    if args.list:
        for file_name, line, name, text in reported:
            mark = "allowed" if (file_name, name) in allowed else "unreachable"
            print(f"{file_name}:{line}: {name}: {text} [{mark}]")
        print(f"{len(reported)} item(s) reported, {len(allowed)} allow-list entr(ies)")
        return

    seen: Set[Item] = {(file_name, name) for file_name, _, name, _ in reported}
    unexpected = [row for row in reported if (row[0], row[2]) not in allowed]
    stale = sorted(key for key in allowed if key not in seen)
    for file_name, line, name, text in unexpected:
        print(f"{file_name}:{line}: unreachable: {name}: {text}")
    for file_name, name in stale:
        print(f"{ALLOW_FILE}: stale entry: {file_name} {name} is no longer reported")
    if unexpected or stale:
        print(
            f"check_dead_code: {len(unexpected)} unreachable item(s), {len(stale)} stale allow-list "
            f"entr(ies). Delete the item, gate it with #[cfg(test)], or list it in {ALLOW_FILE} "
            "with its reason."
        )
        sys.exit(1)
    print(f"check_dead_code: OK ({len(reported)} item(s) reported, all allowed)")


if __name__ == "__main__":
    main()
