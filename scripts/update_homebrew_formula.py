#!/usr/bin/env python3
"""Generates and updates the Homebrew formula for Discipline with verified checksums."""

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


def get_default_version() -> str:
    cargo_toml = Path(__file__).resolve().parent.parent / "Cargo.toml"
    if cargo_toml.exists():
        for line in cargo_toml.read_text(encoding="utf-8").splitlines():
            if line.startswith("version = "):
                return line.split('"')[1]
    return "0.3.0"


def parse_checksums(checksums_path: Path) -> dict[str, str]:
    """Parse SHA256SUMS file into {filename: sha256_hex}."""
    if not checksums_path.exists():
        raise FileNotFoundError(f"Checksums file not found: {checksums_path}")
    mapping = {}
    for line in checksums_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(maxsplit=1)
        if len(parts) == 2:
            sha, fname = parts[0].lower(), Path(parts[1].lstrip("* ")).name
            mapping[fname] = sha
    return mapping


def generate_formula(version: str, checksums: dict[str, str]) -> str:
    """Generate the Homebrew Formula Ruby content with release checksums."""
    # A missing archive is an error, never a placeholder checksum: a formula with a
    # zero digest would install nothing on that platform, silently.
    archives = {
        "darwin_arm": "discipline-aarch64-apple-darwin.tar.gz",
        "darwin_intel": "discipline-x86_64-apple-darwin.tar.gz",
        "linux_arm": "discipline-aarch64-unknown-linux-musl.tar.gz",
        "linux_intel": "discipline-x86_64-unknown-linux-musl.tar.gz",
    }
    missing = [name for name in archives.values() if name not in checksums]
    if missing:
        raise SystemExit(f"error: SHA256SUMS has no entry for {', '.join(missing)}")
    darwin_arm = checksums[archives["darwin_arm"]]
    darwin_intel = checksums[archives["darwin_intel"]]
    linux_arm = checksums[archives["linux_arm"]]
    linux_intel = checksums[archives["linux_intel"]]

    return f"""# typed: false
# frozen_string_literal: true

class Discipline < Formula
  desc "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
  homepage "https://github.com/orieg/discipline"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v{version}/discipline-aarch64-apple-darwin.tar.gz"
      sha256 "{darwin_arm}"
    else
      url "https://github.com/orieg/discipline/releases/download/v{version}/discipline-x86_64-apple-darwin.tar.gz"
      sha256 "{darwin_intel}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v{version}/discipline-aarch64-unknown-linux-musl.tar.gz"
      sha256 "{linux_arm}"
    else
      url "https://github.com/orieg/discipline/releases/download/v{version}/discipline-x86_64-unknown-linux-musl.tar.gz"
      sha256 "{linux_intel}"
    end
  end

  def install
    bin.install "discipline"
    man1.install "man/man1/discipline.1" if File.exist?("man/man1/discipline.1")
    man5.install "man/man5/discipline.toml.5" if File.exist?("man/man5/discipline.toml.5")
  end

  test do
    assert_match "discipline #{{version}}", shell_output("#{{bin}}/discipline --version")
  end
end
"""


def validate_ruby_syntax(content: str) -> bool:
    try:
        proc = subprocess.run(
            ["ruby", "-c"],
            input=content,
            text=True,
            capture_output=True,
            check=False,
        )
        if proc.returncode != 0:
            print(f"Ruby syntax validation failed: {proc.stderr}", file=sys.stderr)
            return False
        return True
    except FileNotFoundError:
        # ruby not installed in environment, skip syntax check
        return True


def push_to_tap_repo(
    tap_repo: str,
    token: str | None,
    deploy_key: str | None,
    formula_content: str,
    version: str,
) -> None:
    """Clone or push formula to the specified Homebrew tap repository via SSH deploy key or HTTPS token."""
    with tempfile.TemporaryDirectory() as tmpdir:
        repo_dir = Path(tmpdir) / "repo"
        git_env = os.environ.copy()
        if deploy_key:
            key_file = Path(tmpdir) / "id_deploy"
            key_file.write_text(deploy_key.strip() + "\n", encoding="utf-8")
            os.chmod(key_file, 0o600)
            git_env["GIT_SSH_COMMAND"] = (
                f"ssh -i {key_file} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"
            )
            if tap_repo.startswith("/") or tap_repo.startswith("file://"):
                clone_url = tap_repo
            else:
                clone_url = f"git@github.com:{tap_repo}.git"
        elif token:
            if tap_repo.startswith("/") or tap_repo.startswith("file://"):
                clone_url = tap_repo
            else:
                clone_url = f"https://x-access-token:{token}@github.com/{tap_repo}.git"
        else:
            raise ValueError("Either deploy_key or token must be provided to push to tap repository")

        print(f"Cloning tap repository {tap_repo}...")
        try:
            subprocess.run(
                ["git", "clone", "--depth", "1", clone_url, str(repo_dir)],
                check=True,
                capture_output=True,
                text=True,
                env=git_env,
            )
        except subprocess.CalledProcessError as e:
            print(f"git clone failed:\nstdout: {e.stdout}\nstderr: {e.stderr}", file=sys.stderr)
            raise

        formula_dir = repo_dir / "Formula"
        formula_dir.mkdir(parents=True, exist_ok=True)
        formula_file = formula_dir / "discipline.rb"
        formula_file.write_text(formula_content, encoding="utf-8")

        subprocess.run(
            ["git", "-C", str(repo_dir), "config", "user.name", "github-actions[bot]"],
            check=True,
            env=git_env,
        )
        subprocess.run(
            [
                "git",
                "-C",
                str(repo_dir),
                "config",
                "user.email",
                "41898282+github-actions[bot]@users.noreply.github.com",
            ],
            check=True,
            env=git_env,
        )
        subprocess.run(
            ["git", "-C", str(repo_dir), "add", "Formula/discipline.rb"],
            check=True,
            env=git_env,
        )

        diff = subprocess.run(
            ["git", "-C", str(repo_dir), "diff", "--staged", "--quiet"],
            check=False,
            env=git_env,
        )
        if diff.returncode == 0:
            print(f"No changes to Formula/discipline.rb in {tap_repo}; already up to date.")
            return

        subprocess.run(
            [
                "git",
                "-C",
                str(repo_dir),
                "commit",
                "-m",
                f"chore(discipline): bump formula to v{version}",
            ],
            check=True,
            env=git_env,
        )
        print(f"Pushing updated formula to {tap_repo}...")
        try:
            subprocess.run(
                ["git", "-C", str(repo_dir), "push", "origin", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
                env=git_env,
            )
        except subprocess.CalledProcessError as e:
            print(f"git push failed:\nstdout: {e.stdout}\nstderr: {e.stderr}", file=sys.stderr)
            raise
        print(f"Successfully pushed updated formula to {tap_repo} for v{version}!")


def main():
    parser = argparse.ArgumentParser(
        description="Generate and update the Homebrew formula for Discipline."
    )
    parser.add_argument(
        "--version",
        default=None,
        help="Release version string without 'v' (defaults to Cargo.toml version)",
    )
    parser.add_argument(
        "--checksums",
        type=Path,
        default=None,
        help="Path to SHA256SUMS file",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Output path for the generated formula (the release attaches it as discipline.rb)",
    )
    parser.add_argument(
        "--push-to-tap",
        default=None,
        help="Target tap repository to push to (e.g. orieg/homebrew-tap)",
    )
    parser.add_argument(
        "--deploy-key",
        default=None,
        help="SSH private key (deploy key) with write access to the tap repository",
    )
    parser.add_argument(
        "--tap-token",
        default=None,
        help="GitHub token with repo write access to the tap repository",
    )

    args = parser.parse_args()
    version = args.version or get_default_version()
    version = version.lstrip("v")

    checksums = {}
    if args.checksums:
        if not args.checksums.exists():
            print(f"Error: checksums file not found: {args.checksums}", file=sys.stderr)
            sys.exit(1)
        checksums = parse_checksums(args.checksums)
    elif Path("dist/SHA256SUMS").exists():
        checksums = parse_checksums(Path("dist/SHA256SUMS"))

    content = generate_formula(version, checksums)

    if not validate_ruby_syntax(content):
        sys.exit(1)

    if args.output is None and not args.push_to_tap:
        print("Error: pass --output and/or --push-to-tap", file=sys.stderr)
        sys.exit(2)
    out_file = args.output
    if out_file is not None:
        out_file.parent.mkdir(parents=True, exist_ok=True)
        out_file.write_text(content, encoding="utf-8")
    if out_file is not None:
        print(f"Generated Homebrew formula (v{version}) at {out_file}")

    if args.push_to_tap:
        deploy_key = args.deploy_key or os.environ.get("HOMEBREW_TAP_DEPLOY_KEY")
        token = args.tap_token or os.environ.get("HOMEBREW_TAP_TOKEN")
        if not deploy_key and not token:
            print(
                f"::notice::Push to {args.push_to_tap} requested but neither deploy key nor token provided; skipping tap push.",
                file=sys.stderr,
            )
        else:
            push_to_tap_repo(args.push_to_tap, token, deploy_key, content, version)


if __name__ == "__main__":
    main()
