#!/usr/bin/env python3
"""Builds a standard APT repository from a directory of .deb packages and generates an index.html."""

import argparse
import datetime
import gzip
import hashlib
import io
import os
import shutil
import subprocess
import tarfile

from site_theme import (
    BASE_CSS,
    COPY_BTN_CSS,
    NAV_CSS,
    SITE_JS,
    THEME_CSS_VARS,
    THEME_HEAD_JS,
    THEME_TOGGLE_CSS,
    THEME_TOGGLE_JS,
    make_nav,
)


def get_default_version() -> str:
    cargo_toml = os.path.join(os.path.dirname(__file__), "..", "Cargo.toml")
    if os.path.isfile(cargo_toml):
        with open(cargo_toml, "r", encoding="utf-8") as f:
            for line in f:
                if line.startswith("version ="):
                    return line.split("=")[1].strip().strip('"')
    return "0.3.0"


def sha256_file(filepath: str) -> str:
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def extract_control_info(deb_path: str) -> dict:
    info = {}

    # 1. Try dpkg-deb if available
    try:
        res = subprocess.run(
            ["dpkg-deb", "-f", deb_path],
            capture_output=True,
            text=True,
            check=True,
        )
        for line in res.stdout.splitlines():
            if ":" in line:
                k, v = line.split(":", 1)
                info[k.strip().lower()] = v.strip()
        if "package" in info:
            return info
    except Exception:
        pass

    # 2. Native 'ar' format extraction
    try:
        with open(deb_path, "rb") as f:
            magic = f.read(8)
            if magic == b"!<arch>\n":
                while True:
                    header = f.read(60)
                    if len(header) < 60:
                        break
                    member_name = (
                        header[0:16]
                        .decode("ascii", errors="ignore")
                        .strip()
                        .rstrip("/")
                    )
                    member_size = int(
                        header[48:58].decode("ascii", errors="ignore").strip()
                    )
                    member_data = f.read(member_size)
                    if member_size % 2 != 0:
                        f.read(1)  # ar padding

                    if member_name in (
                        "control.tar.gz",
                        "control.tar.xz",
                        "control.tar.zst",
                        "control.tar",
                    ):
                        with tarfile.open(
                            fileobj=io.BytesIO(member_data), mode="r:*"
                        ) as ctar:
                            for cm in ctar.getmembers():
                                if cm.name in ("control", "./control"):
                                    cf = ctar.extractfile(cm)
                                    if cf:
                                        for line in (
                                            cf.read()
                                            .decode("utf-8", errors="ignore")
                                            .splitlines()
                                        ):
                                            if ":" in line:
                                                k, v = line.split(":", 1)
                                                info[k.strip().lower()] = v.strip()
                        if "package" in info:
                            return info
    except Exception:
        pass

    # 3. Direct tarfile fallback
    try:
        with tarfile.open(deb_path, "r:*") as ar:
            for m in ar.getmembers():
                if m.name in (
                    "control.tar.gz",
                    "./control.tar.gz",
                    "control.tar.xz",
                    "./control.tar.xz",
                    "control.tar.zst",
                    "./control.tar.zst",
                ):
                    cf_extracted = ar.extractfile(m)
                    if cf_extracted:
                        with tarfile.open(fileobj=cf_extracted, mode="r:*") as ctar:
                            for cm in ctar.getmembers():
                                if cm.name in ("control", "./control"):
                                    cf = ctar.extractfile(cm)
                                    if cf:
                                        for line in (
                                            cf.read()
                                            .decode("utf-8", errors="ignore")
                                            .splitlines()
                                        ):
                                            if ":" in line:
                                                k, v = line.split(":", 1)
                                                info[k.strip().lower()] = v.strip()
    except Exception:
        pass

    return info


def build_apt_repo(
    input_dir: str,
    output_dir: str,
    allow_empty: bool = False,
    version: str = "0.3.0",
):
    deb_files = []
    if os.path.isdir(input_dir):
        for root, _, files in os.walk(input_dir):
            for file in files:
                if file.endswith(".deb"):
                    deb_files.append(os.path.join(root, file))

    if not allow_empty:
        if not os.path.isdir(input_dir):
            raise SystemExit(
                f"::error::APT repo build: artifacts directory '{input_dir}' does not exist. "
                f"Download the release .deb assets first, or pass --allow-empty to bootstrap an empty repo."
            )
        if not deb_files:
            raise SystemExit(
                f"::error::APT repo build: no .deb packages found under '{input_dir}'. "
                f"Refusing to publish an empty APT repository. Pass --allow-empty to bootstrap intentionally."
            )

    os.makedirs(output_dir, exist_ok=True)
    pool_dir = os.path.join(output_dir, "pool", "main")
    os.makedirs(pool_dir, exist_ok=True)

    arch_packages = {"amd64": [], "arm64": []}

    for deb_path in deb_files:
        filename = os.path.basename(deb_path)
        dest_path = os.path.join(pool_dir, filename)
        shutil.copy2(deb_path, dest_path)

        size = os.path.getsize(dest_path)
        sha256 = sha256_file(dest_path)
        control = extract_control_info(dest_path)

        pkg_name = control.get(
            "package", filename.split("_")[0] if "_" in filename else "discipline"
        )
        pkg_version = control.get(
            "version", filename.split("_")[1] if "_" in filename else version
        )
        arch = control.get(
            "architecture",
            filename.split("_")[2].replace(".deb", "")
            if "_" in filename
            else "amd64",
        )
        maintainer = control.get(
            "maintainer", "Discipline Contributors <https://github.com/orieg/discipline>"
        )
        description = control.get(
            "description", "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
        )
        depends = control.get("depends", "")
        provides = control.get("provides", "")
        conflicts = control.get("conflicts", "")

        entry = {
            "Package": pkg_name,
            "Version": pkg_version,
            "Architecture": arch,
            "Maintainer": maintainer,
            "Description": description,
            "Filename": f"pool/main/{filename}",
            "Size": str(size),
            "SHA256": sha256,
        }
        if depends:
            entry["Depends"] = depends
        if provides:
            entry["Provides"] = provides
        if conflicts:
            entry["Conflicts"] = conflicts

        if arch in arch_packages:
            arch_packages[arch].append(entry)
        else:
            arch_packages.setdefault(arch, []).append(entry)

    dists_dir = os.path.join(output_dir, "dists", "stable")
    os.makedirs(dists_dir, exist_ok=True)
    file_manifests = []

    for arch, pkgs in arch_packages.items():
        arch_dir = os.path.join(dists_dir, "main", f"binary-{arch}")
        os.makedirs(arch_dir, exist_ok=True)

        pkg_file_path = os.path.join(arch_dir, "Packages")
        with open(pkg_file_path, "w", encoding="utf-8") as f:
            for pkg in pkgs:
                for k, v in pkg.items():
                    f.write(f"{k}: {v}\n")
                f.write("\n")

        gz_file_path = f"{pkg_file_path}.gz"
        with open(pkg_file_path, "rb") as f_in, gzip.open(gz_file_path, "wb") as f_out:
            shutil.copyfileobj(f_in, f_out)

        rel_plain = os.path.relpath(pkg_file_path, dists_dir)
        rel_gz = os.path.relpath(gz_file_path, dists_dir)

        file_manifests.append(
            (rel_plain, os.path.getsize(pkg_file_path), sha256_file(pkg_file_path))
        )
        file_manifests.append(
            (rel_gz, os.path.getsize(gz_file_path), sha256_file(gz_file_path))
        )

    release_path = os.path.join(dists_dir, "Release")
    now_utc = datetime.datetime.now(datetime.timezone.utc).strftime(
        "%a, %d %b %Y %H:%M:%S UTC"
    )

    with open(release_path, "w", encoding="utf-8") as f:
        f.write("Origin: Discipline\n")
        f.write("Label: Discipline\n")
        f.write("Suite: stable\n")
        f.write("Codename: stable\n")
        f.write("Architectures: amd64 arm64\n")
        f.write("Components: main\n")
        f.write("Description: Official Discipline APT Repository\n")
        f.write(f"Date: {now_utc}\n")
        f.write("SHA256:\n")
        for rel_path, size, sha256 in file_manifests:
            f.write(f" {sha256} {size} {rel_path}\n")

    html_content = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Discipline — Official APT Repository</title>
  <meta name="description" content="Official Debian and Ubuntu APT repository for Discipline — universal CI/CD diff sentinel and AI coding agent gatekeeper.">
  <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>&#x1f6e1;&#xfe0f;</text></svg>">
{THEME_HEAD_JS}
  <style>
{THEME_CSS_VARS}{BASE_CSS}{NAV_CSS}{THEME_TOGGLE_CSS}{COPY_BTN_CSS}
    .container {{
      max-width: 900px;
      margin: 2rem auto;
      padding: 0 1.5rem;
    }}
    h1 {{
      font-size: clamp(1.6rem, 4vw, 2.2rem);
      font-weight: 800;
      color: var(--heading);
      margin-bottom: 0.5rem;
      letter-spacing: -0.02em;
    }}
    h2 {{
      font-size: 1.25rem;
      font-weight: 700;
      color: var(--heading);
      margin-bottom: 0.75rem;
    }}
    .card {{
      background: var(--card-bg);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 1.5rem;
      margin-top: 1.5rem;
      box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
    }}
    pre {{
      background: var(--code-bg);
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 1.25rem;
      overflow-x: auto;
      color: #7dd3fc;
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 0.88rem;
      line-height: 1.6;
      margin-top: 0.75rem;
    }}
    code {{
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 0.88em;
      color: var(--accent);
    }}
    .table-responsive {{
      overflow-x: auto;
      width: 100%;
      -webkit-overflow-scrolling: touch;
      margin-top: 0.75rem;
    }}
    table {{
      width: 100%;
      border-collapse: collapse;
      font-size: 0.92rem;
      min-width: 480px;
    }}
    th, td {{
      padding: 0.75rem 1rem;
      text-align: left;
      border-bottom: 1px solid var(--table-row-border);
    }}
    th {{
      font-weight: 600;
      color: var(--table-header-color);
      background: var(--card-inner);
    }}
    td {{ color: var(--text); }}
    a {{
      color: var(--accent);
      text-decoration: none;
    }}
    a:hover {{ text-decoration: underline; }}
    .badge {{
      display: inline-block;
      padding: 0.25rem 0.6rem;
      font-size: 0.75rem;
      font-weight: 600;
      border-radius: 12px;
      background: var(--badge-bg);
      border: 1px solid var(--badge-border);
      color: var(--badge-text);
      vertical-align: middle;
      margin-left: 0.5rem;
    }}
    footer {{
      border-top: 1px solid var(--border);
      margin-top: 3rem;
      padding: 2rem 0;
      text-align: center;
      color: var(--text-muted);
      font-size: 0.85rem;
    }}

    @media (max-width: 768px) {{
      .container {{ padding: 0 1rem; margin: 1.5rem auto; }}
      .card {{ padding: 1.15rem; }}
      pre {{ padding: 0.9rem; font-size: 0.82rem; }}
    }}
  </style>
</head>
<body>
{make_nav(version, "apt", base="../")}

  <main>
  <div class="container">
    <h1>Discipline APT Repository <span class="badge">v{version}</span></h1>
    <p style="color: var(--text-muted);">Official Debian and Ubuntu package repository for <strong><a href="https://github.com/orieg/discipline">Discipline</a></strong> &mdash; universal CI/CD diff sentinel and AI coding agent gatekeeper.</p>

    <div class="card">
      <h2>Quick Setup</h2>
      <p style="color: var(--text-muted); margin-top: 0.5rem;">Add the official repository source to your system:</p>
      <pre><code># 1. Add repository source
echo "deb [trusted=yes] https://orieg.github.io/discipline/apt/ stable main" | sudo tee /etc/apt/sources.list.d/discipline.list

# 2. Update apt cache and install discipline
sudo apt-get update
sudo apt-get install -y discipline</code></pre>
    </div>

    <div class="card">
      <h2>Packages in this Repository</h2>
      <div class="table-responsive">
        <table>
          <thead>
            <tr>
              <th>Package</th>
              <th>Architecture</th>
              <th>Direct Download</th>
            </tr>
          </thead>
          <tbody>
"""
    has_pkgs = False
    for arch, pkgs in arch_packages.items():
        for pkg in pkgs:
            has_pkgs = True
            fname = pkg["Filename"]
            pname = pkg["Package"]
            html_content += f"""            <tr>
              <td><strong>{pname}</strong></td>
              <td><code>{arch}</code></td>
              <td><a href="{fname}">{os.path.basename(fname)}</a></td>
            </tr>\n"""

    if not has_pkgs:
        html_content += f"""            <tr>
              <td><strong>discipline</strong></td>
              <td><code>amd64</code></td>
              <td><a href="pool/main/discipline_{version}_amd64.deb">discipline_{version}_amd64.deb</a></td>
            </tr>
            <tr>
              <td><strong>discipline</strong></td>
              <td><code>arm64</code></td>
              <td><a href="pool/main/discipline_{version}_arm64.deb">discipline_{version}_arm64.deb</a></td>
            </tr>\n"""

    html_content += f"""          </tbody>
        </table>
      </div>
    </div>

    <div class="card">
      <h2>Supported Architectures</h2>
      <ul style="margin-left: 1.5rem; margin-top: 0.5rem; line-height: 1.8;">
        <li><strong>amd64 (x86_64)</strong>: Intel/AMD 64-bit Linux (Debian 11+, Ubuntu 20.04+).</li>
        <li><strong>arm64 (aarch64)</strong>: ARM 64-bit Linux (AWS Graviton, Raspberry Pi 4/5, Apple Silicon Linux VMs).</li>
      </ul>
    </div>

    <div class="card">
      <h2>Repository Metadata Files</h2>
      <p style="color: var(--text-muted); margin-top: 0.5rem;">Direct links to repository configuration and metadata files:</p>
      <ul style="margin-left: 1.5rem; margin-top: 0.5rem; line-height: 1.8;">
        <li><a href="dists/stable/Release"><code>dists/stable/Release</code></a> &mdash; Repository Release manifest</li>
        <li><a href="dists/stable/main/binary-amd64/Packages.gz"><code>dists/stable/main/binary-amd64/Packages.gz</code></a> &mdash; amd64 package index</li>
        <li><a href="dists/stable/main/binary-arm64/Packages.gz"><code>dists/stable/main/binary-arm64/Packages.gz</code></a> &mdash; arm64 package index</li>
      </ul>
    </div>

    <footer>
      <p style="margin-bottom: 0.75rem; color: var(--text-muted); font-size: 0.85rem;">
        Judy &amp; Systems Ecosystem:
        <a href="https://orieg.github.io/judy-cache/">Judy Cache PSR-16</a> &bull;
        <a href="https://orieg.github.io/php-judy/">PHP Judy</a> &bull;
        <a href="https://orieg.github.io/judy-polyfill/">Judy Polyfill</a> &bull;
        <a href="https://orieg.github.io/expanse/">Expanse Engine</a> &bull;
        <a href="https://orieg.github.io/">Nicolas Brousse (Hub)</a>
      </p>
      <p>Maintained by <a href="https://nicolas.brousse.info/">Nicolas Brousse</a> &bull; <a href="https://github.com/orieg/discipline">GitHub</a> &bull; <a href="https://crates.io/crates/discipline">Crates.io</a> &bull; <a href="../rpm/">RPM Repo</a></p>
    </footer>
  </div>
  </main>

  {THEME_TOGGLE_JS}
  {SITE_JS}
</body>
</html>
"""
    with open(os.path.join(output_dir, "index.html"), "w", encoding="utf-8") as f:
        f.write(html_content)

    print(f"APT repository and index.html successfully generated in {output_dir}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Build APT repository.")
    parser.add_argument("in_pos", nargs="?", default=None)
    parser.add_argument("out_pos", nargs="?", default=None)
    parser.add_argument("--artifacts-dir", "--input-dir", dest="in_dir", default=None)
    parser.add_argument("--output-dir", dest="out_dir", default=None)
    parser.add_argument(
        "--allow-empty",
        action="store_true",
        help="Permit building an empty repository (bootstrap only) instead of failing.",
    )
    parser.add_argument("--version", dest="version", default=None)
    args = parser.parse_args()

    in_dir = args.in_dir or args.in_pos or "artifacts"
    out_dir = args.out_dir or args.out_pos or "docs/apt"
    ver = args.version or get_default_version()
    build_apt_repo(in_dir, out_dir, allow_empty=args.allow_empty, version=ver)
