#!/usr/bin/env python3
"""Builds both APT and RPM repositories for Discipline."""

import argparse
import os
import sys

# Ensure local scripts directory is in sys.path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from build_apt_repo import build_apt_repo, get_default_version
from build_rpm_repo import build_rpm_repo


def main():
    parser = argparse.ArgumentParser(
        description="Build both APT and RPM package repositories for Discipline."
    )
    parser.add_argument(
        "--artifacts-dir",
        default="artifacts",
        help="Input directory containing .deb and .rpm release packages (default: artifacts)",
    )
    parser.add_argument(
        "--docs-dir",
        default="docs",
        help="Target docs root directory containing apt/ and rpm/ (default: docs)",
    )
    parser.add_argument(
        "--allow-empty",
        action="store_true",
        help="Permit building initial repository metadata when artifacts directory has no packages yet.",
    )
    parser.add_argument(
        "--version",
        default=None,
        help="Package version string (defaults to Cargo.toml version).",
    )
    args = parser.parse_args()

    ver = args.version or get_default_version()
    apt_out = os.path.join(args.docs_dir, "apt")
    rpm_out = os.path.join(args.docs_dir, "rpm")

    print(f"Building Discipline APT repository (v{ver}) into {apt_out}...")
    build_apt_repo(args.artifacts_dir, apt_out, allow_empty=args.allow_empty, version=ver)

    print(f"Building Discipline RPM repository (v{ver}) into {rpm_out}...")
    build_rpm_repo(args.artifacts_dir, rpm_out, allow_empty=args.allow_empty, version=ver)

    print("All package repositories successfully generated.")


if __name__ == "__main__":
    main()
