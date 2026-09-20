#!/usr/bin/env python3
"""Builds an RPM repository with repodata metadata, discipline.repo, and a responsive index.html."""

import argparse
import datetime
import gzip
import hashlib
import os
import shutil
import subprocess

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
    return "0.2.2"


def sha256_file(filepath: str) -> str:
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def build_rpm_repo(
    input_dir: str,
    output_dir: str,
    allow_empty: bool = False,
    version: str = "0.2.2",
):
    rpm_files = []
    if os.path.isdir(input_dir):
        for root, _, files in os.walk(input_dir):
            for file in files:
                if file.endswith(".rpm"):
                    rpm_files.append(os.path.join(root, file))

    if not allow_empty:
        if not os.path.isdir(input_dir):
            raise SystemExit(
                f"::error::RPM repo build: artifacts directory '{input_dir}' does not exist. "
                f"Download the release .rpm assets first, or pass --allow-empty to bootstrap an empty repo."
            )
        if not rpm_files:
            raise SystemExit(
                f"::error::RPM repo build: no .rpm packages found under '{input_dir}'. "
                f"Refusing to publish an empty RPM repository. Pass --allow-empty to bootstrap intentionally."
            )

    os.makedirs(output_dir, exist_ok=True)
    repodata_dir = os.path.join(output_dir, "repodata")
    os.makedirs(repodata_dir, exist_ok=True)
    packages_dir = os.path.join(output_dir, "Packages")
    os.makedirs(packages_dir, exist_ok=True)

    arch_packages = {"x86_64": [], "aarch64": []}

    for rpm_path in rpm_files:
        filename = os.path.basename(rpm_path)
        arch = "x86_64"
        if "aarch64" in filename or "arm64" in filename:
            arch = "aarch64"

        target_arch_dir = os.path.join(packages_dir, arch)
        os.makedirs(target_arch_dir, exist_ok=True)
        dest_path = os.path.join(target_arch_dir, filename)
        shutil.copy2(rpm_path, dest_path)

        size = os.path.getsize(dest_path)
        sha256 = sha256_file(dest_path)
        pkg_name = filename.split("-")[0]
        pkg_ver = version

        entry = {
            "name": pkg_name,
            "arch": arch,
            "version": pkg_ver,
            "release": "1",
            "filename": f"Packages/{arch}/{filename}",
            "size": size,
            "sha256": sha256,
        }
        arch_packages[arch].append(entry)

    has_createrepo = shutil.which("createrepo_c") or shutil.which("createrepo")
    if has_createrepo and any(arch_packages.values()):
        subprocess.run([has_createrepo, "--update", output_dir], check=True)
    else:
        now_ts = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
        pkg_count = sum(len(v) for v in arch_packages.values())
        primary_xml = f'<?xml version="1.0" encoding="UTF-8"?>\n<metadata xmlns="http://linux.duke.edu/metadata/common" packages="{pkg_count}">\n'
        for arch, pkgs in arch_packages.items():
            for p in pkgs:
                primary_xml += f"""  <package type="rpm">
    <name>{p['name']}</name>
    <arch>{p['arch']}</arch>
    <version epoch="0" ver="{p['version']}" rel="{p['release']}"/>
    <checksum type="sha256" pkgid="YES">{p['sha256']}</checksum>
    <summary>Universal CI/CD gatekeeper and AI coding agent diff sentinel</summary>
    <description>Discipline transforms verification rigors from high-assurance algorithm repositories into a single, declarative static binary wrapped in a composite action and standalone CLI.</description>
    <packager>Discipline Contributors &lt;https://github.com/orieg/discipline&gt;</packager>
    <url>https://github.com/orieg/discipline</url>
    <time file="{now_ts}" build="{now_ts}"/>
    <size package="{p['size']}" installed="{p['size']}" archive="{p['size']}"/>
    <location href="{p['filename']}"/>
    <format>
      <provides>
        <entry name="{p['name']}" flags="EQ" epoch="0" ver="{p['version']}" rel="{p['release']}"/>
      </provides>
    </format>
  </package>\n"""
        primary_xml += "</metadata>\n"

        primary_path = os.path.join(repodata_dir, "primary.xml")
        primary_gz_path = os.path.join(repodata_dir, "primary.xml.gz")
        with open(primary_path, "w", encoding="utf-8") as f:
            f.write(primary_xml)
        with open(primary_path, "rb") as f_in, gzip.open(
            primary_gz_path, "wb"
        ) as f_out:
            shutil.copyfileobj(f_in, f_out)
        os.remove(primary_path)

        filelists_xml = f'<?xml version="1.0" encoding="UTF-8"?>\n<filelists xmlns="http://linux.duke.edu/metadata/filelists" packages="{pkg_count}">\n</filelists>\n'
        filelists_gz_path = os.path.join(repodata_dir, "filelists.xml.gz")
        with gzip.open(filelists_gz_path, "wt", encoding="utf-8") as f_out:
            f_out.write(filelists_xml)

        other_xml = f'<?xml version="1.0" encoding="UTF-8"?>\n<otherdata xmlns="http://linux.duke.edu/metadata/other" packages="{pkg_count}">\n</otherdata>\n'
        other_gz_path = os.path.join(repodata_dir, "other.xml.gz")
        with gzip.open(other_gz_path, "wt", encoding="utf-8") as f_out:
            f_out.write(other_xml)

        primary_size = os.path.getsize(primary_gz_path)
        primary_sha256 = sha256_file(primary_gz_path)
        filelists_size = os.path.getsize(filelists_gz_path)
        filelists_sha256 = sha256_file(filelists_gz_path)
        other_size = os.path.getsize(other_gz_path)
        other_sha256 = sha256_file(other_gz_path)

        repomd_xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<repomd xmlns="http://linux.duke.edu/metadata/repo">
  <revision>{now_ts}</revision>
  <data type="primary">
    <checksum type="sha256">{primary_sha256}</checksum>
    <location href="repodata/primary.xml.gz"/>
    <timestamp>{now_ts}</timestamp>
    <size>{primary_size}</size>
  </data>
  <data type="filelists">
    <checksum type="sha256">{filelists_sha256}</checksum>
    <location href="repodata/filelists.xml.gz"/>
    <timestamp>{now_ts}</timestamp>
    <size>{filelists_size}</size>
  </data>
  <data type="other">
    <checksum type="sha256">{other_sha256}</checksum>
    <location href="repodata/other.xml.gz"/>
    <timestamp>{now_ts}</timestamp>
    <size>{other_size}</size>
  </data>
</repomd>
"""
        with open(os.path.join(repodata_dir, "repomd.xml"), "w", encoding="utf-8") as f:
            f.write(repomd_xml)

    repo_file_content = """[discipline]
name=Discipline Enterprise Linux Repository
baseurl=https://orieg.github.io/discipline/rpm/
enabled=1
gpgcheck=0
repo_gpgcheck=0
"""
    with open(os.path.join(output_dir, "discipline.repo"), "w", encoding="utf-8") as f:
        f.write(repo_file_content)

    html_content = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Discipline — Official RPM Repository</title>
  <meta name="description" content="Official RPM repository for Discipline — universal CI/CD diff sentinel and AI coding agent gatekeeper.">
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
{make_nav(version, "rpm", base="../")}

  <main>
  <div class="container">
    <h1>Discipline RPM Repository <span class="badge">v{version}</span></h1>
    <p style="color: var(--text-muted);">Official YUM / DNF repository for <strong><a href="https://github.com/orieg/discipline">Discipline</a></strong> across Enterprise Linux: RHEL 8/9/10, CentOS Stream, Fedora, Rocky Linux, AlmaLinux, and Amazon Linux 2023.</p>

    <div class="card">
      <h2>Quick Setup</h2>
      <p style="color: var(--text-muted); margin-top: 0.5rem;">Configure the official repository using DNF / YUM:</p>
      <pre><code># 1. Add repository configuration
sudo dnf config-manager --add-repo https://orieg.github.io/discipline/rpm/discipline.repo

# Or manually download repo file for older YUM / Amazon Linux:
# sudo curl -sS -o /etc/yum.repos.d/discipline.repo https://orieg.github.io/discipline/rpm/discipline.repo

# 2. Install discipline binary
sudo dnf install -y discipline</code></pre>
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
            fname = pkg["filename"]
            pname = pkg["name"]
            html_content += f"""            <tr>
              <td><strong>{pname}</strong></td>
              <td><code>{arch}</code></td>
              <td><a href="{fname}">{os.path.basename(fname)}</a></td>
            </tr>\n"""

    if not has_pkgs:
        html_content += f"""            <tr>
              <td><strong>discipline</strong></td>
              <td><code>x86_64</code></td>
              <td><a href="Packages/x86_64/discipline-{version}-1.x86_64.rpm">discipline-{version}-1.x86_64.rpm</a></td>
            </tr>
            <tr>
              <td><strong>discipline</strong></td>
              <td><code>aarch64</code></td>
              <td><a href="Packages/aarch64/discipline-{version}-1.aarch64.rpm">discipline-{version}-1.aarch64.rpm</a></td>
            </tr>\n"""

    html_content += f"""          </tbody>
        </table>
      </div>
    </div>

    <div class="card">
      <h2>Supported Platforms &amp; Architectures</h2>
      <ul style="margin-left: 1.5rem; margin-top: 0.5rem; line-height: 1.8;">
        <li><strong>x86_64</strong>: Enterprise Linux 8/9/10, Fedora, CentOS Stream, Rocky Linux, AlmaLinux.</li>
        <li><strong>aarch64 (ARM64)</strong>: AWS Graviton, Ampere Altra, ARM64 servers and containers.</li>
      </ul>
    </div>

    <div class="card">
      <h2>Repository Metadata Files</h2>
      <p style="color: var(--text-muted); margin-top: 0.5rem;">Direct links to repository configuration and repomd metadata:</p>
      <ul style="margin-left: 1.5rem; margin-top: 0.5rem; line-height: 1.8;">
        <li><a href="discipline.repo"><code>discipline.repo</code></a> &mdash; YUM/DNF repository configuration</li>
        <li><a href="repodata/repomd.xml"><code>repodata/repomd.xml</code></a> &mdash; Repository index metadata</li>
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
      <p>Maintained by <a href="https://nicolas.brousse.info/">Nicolas Brousse</a> &bull; <a href="https://github.com/orieg/discipline">GitHub</a> &bull; <a href="https://crates.io/crates/discipline">Crates.io</a> &bull; <a href="../apt/">APT Repo</a></p>
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

    print(f"RPM repository and index.html successfully generated in {output_dir}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Build RPM repository.")
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
    out_dir = args.out_dir or args.out_pos or "docs/rpm"
    ver = args.version or get_default_version()
    build_rpm_repo(in_dir, out_dir, allow_empty=args.allow_empty, version=ver)
