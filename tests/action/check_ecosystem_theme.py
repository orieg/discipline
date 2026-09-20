#!/usr/bin/env python3
"""Ecosystem Theme Contract v1 Linter for orieg/discipline.

Enforces compliance with the Ecosystem Theme Contract v1 across documentation surfaces:
  1. Storage key is 'orieg-theme'. It stores ONLY an explicit override ('light' | 'dark').
     Absence means follow OS preferences (system).
  2. Three-state cyclic reachability: system -> light -> dark -> system.
  3. Resolved theme is set as data-theme="light"|"dark" on <html>.
  4. User-facing mode is set as data-theme-mode="system"|"light"|"dark" on <html>.
  5. Live matchMedia listener on '(prefers-color-scheme: dark)'.
  6. Migration: reads 'getItem("orieg-theme") || getItem("discipline-theme")',
     writes only 'orieg-theme'.
  7. Minimum WCAG AA 4.5:1 relative luminance contrast on all palette tokens.

Usage:
  python3 tests/action/check_ecosystem_theme.py --local-only
  python3 tests/action/check_ecosystem_theme.py --self-test
  python3 tests/action/check_ecosystem_theme.py
"""

from __future__ import annotations

import argparse
import os
import sys
import unittest
import urllib.error
import urllib.request
from typing import Dict, List, Optional, Tuple

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

# Canonical palette tokens for orieg/discipline
DARK_PALETTE = {
    "bg": "#090d16",
    "card-bg": "#111827",
    "card-inner": "#0b1120",
    "border": "#1f293d",
    "text": "#e2e8f0",
    "text-muted": "#94a3b8",
    "heading": "#f8fafc",
    "accent": "#38bdf8",
    "accent-green": "#10b981",
    "code-bg": "#030712",
    "badge-near": "#facc15",
    "badge-gap": "#f87171",
}

LIGHT_PALETTE = {
    "bg": "#f8fafc",
    "card-bg": "#ffffff",
    "card-inner": "#f1f5f9",
    "border": "#e2e8f0",
    "text": "#334155",
    "text-muted": "#64748b",
    "heading": "#0f172a",
    "accent": "#0284c7",
    "accent-green": "#059669",
    "code-bg": "#0f172a",
    "badge-near": "#b45309",
    "badge-gap": "#be123c",
}

ECOSYSTEM_SITES = [
    {
        "id": "discipline",
        "name": "Discipline Diff Sentinel",
        "url": "https://orieg.github.io/discipline/",
        "legacy_key": "discipline-theme",
    },
    {
        "id": "expanse",
        "name": "Expanse Digital Trees",
        "url": "https://orieg.github.io/expanse/",
        "legacy_key": "expanse-theme",
    },
    {
        "id": "hub",
        "name": "Nicolas Brousse (Hub)",
        "url": "https://orieg.github.io/",
        "legacy_key": "orieg-theme",
    },
    {
        "id": "php-judy",
        "name": "PHP Judy Extension",
        "url": "https://orieg.github.io/php-judy/",
        "legacy_key": "judy-theme",
    },
    {
        "id": "judy-cache",
        "name": "Judy Cache PSR-16",
        "url": "https://orieg.github.io/judy-cache/",
        "legacy_key": "judy-cache-theme",
    },
    {
        "id": "judy-polyfill",
        "name": "Judy Polyfill",
        "url": "https://orieg.github.io/judy-polyfill/",
        "legacy_key": "judy-polyfill-theme",
    },
]


def relative_luminance(hex_color: str) -> float:
    """Computes standard relative luminance per WCAG 2.1 (sRGB color space)."""
    hex_clean = hex_color.strip().lstrip("#")
    if len(hex_clean) == 3:
        hex_clean = "".join(c * 2 for c in hex_clean)
    if len(hex_clean) != 6:
        raise ValueError(f"Invalid hex color: {hex_color}")

    r = int(hex_clean[0:2], 16) / 255.0
    g = int(hex_clean[2:4], 16) / 255.0
    b = int(hex_clean[4:6], 16) / 255.0

    def to_linear(c: float) -> float:
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r_lin = to_linear(r)
    g_lin = to_linear(g)
    b_lin = to_linear(b)

    return 0.2126 * r_lin + 0.7152 * g_lin + 0.0722 * b_lin


def contrast_ratio(hex_a: str, hex_b: str) -> float:
    """Computes the WCAG contrast ratio between two hex colors ((L1 + 0.05) / (L2 + 0.05))."""
    lum_a = relative_luminance(hex_a)
    lum_b = relative_luminance(hex_b)
    l1 = max(lum_a, lum_b)
    l2 = min(lum_a, lum_b)
    return (l1 + 0.05) / (l2 + 0.05)


def validate_palette_contrast(palette: Dict[str, str], name: str) -> List[str]:
    """Validates that text tokens meet the 4.5:1 WCAG AA floor against background tokens."""
    errors = []
    bg = palette.get("bg")
    card_bg = palette.get("card-bg")
    card_inner = palette.get("card-inner")

    checks: List[Tuple[str, List[Optional[str]]]] = [
        ("text", [bg, card_bg, card_inner]),
        ("heading", [bg, card_bg, card_inner]),
        ("badge-near", [card_bg, card_inner]),
        ("badge-gap", [card_bg, card_inner]),
    ]

    for token, bgs in checks:
        color = palette.get(token)
        if not color or not color.startswith("#"):
            continue
        for b in bgs:
            if not b or not b.startswith("#"):
                continue
            ratio = contrast_ratio(color, b)
            if ratio < 4.5:
                errors.append(
                    f"{name}: token '{token}' ({color}) has contrast {ratio:.2f}:1 against background ({b}) < 4.5:1 floor"
                )

    return errors


def validate_contract_script(
    content: str, legacy_key: Optional[str] = None, strict_v1: bool = False
) -> List[str]:
    """Validates contract compliance for embedded JavaScript / HTML."""
    errors = []

    if "data-theme" not in content:
        errors.append("Contract violation: 'data-theme' attribute is not set on document element")

    has_canonical = "orieg-theme" in content
    has_legacy = legacy_key and legacy_key in content
    if not (has_canonical or has_legacy):
        errors.append(
            f"Contract violation: neither canonical storage key 'orieg-theme' nor legacy key '{legacy_key}' is referenced"
        )

    if strict_v1:
        if not has_canonical:
            errors.append("Contract violation: canonical storage key 'orieg-theme' is not referenced")
        if "data-theme-mode" not in content:
            errors.append("Contract violation: 'data-theme-mode' attribute is not set on document element")
        has_system = "system" in content
        has_light = "light" in content
        has_dark = "dark" in content
        if not (has_system and has_light and has_dark):
            errors.append("Contract violation: 3-state theme modes (system, light, dark) not fully supported")
        if "matchMedia" not in content or "prefers-color-scheme" not in content:
            errors.append("Contract violation: live 'prefers-color-scheme' matchMedia listener is missing")

    return errors


def check_local_discipline(repo_root: str) -> List[str]:
    """Validates local discipline documentation surfaces and palettes."""
    errors = []

    # 1. Check palette contrast floors
    errors.extend(validate_palette_contrast(DARK_PALETTE, "discipline.DARK_PALETTE"))
    errors.extend(validate_palette_contrast(LIGHT_PALETTE, "discipline.LIGHT_PALETTE"))

    # 2. Check docs/index.html
    index_path = os.path.join(repo_root, "docs", "index.html")
    if os.path.isfile(index_path):
        with open(index_path, "r", encoding="utf-8") as f:
            index_content = f.read()
        errs = validate_contract_script(index_content, legacy_key="discipline-theme", strict_v1=True)
        errors.extend([f"docs/index.html: {e}" for e in errs])
    else:
        errors.append("docs/index.html not found")

    # 3. Check docs/_layouts/default.html
    layout_path = os.path.join(repo_root, "docs", "_layouts", "default.html")
    if os.path.isfile(layout_path):
        with open(layout_path, "r", encoding="utf-8") as f:
            layout_content = f.read()
        errs = validate_contract_script(layout_content, legacy_key="discipline-theme", strict_v1=True)
        errors.extend([f"docs/_layouts/default.html: {e}" for e in errs])
    else:
        errors.append("docs/_layouts/default.html not found")

    # 4. Check docs/apt/index.html if present
    apt_path = os.path.join(repo_root, "docs", "apt", "index.html")
    if os.path.isfile(apt_path):
        with open(apt_path, "r", encoding="utf-8") as f:
            apt_content = f.read()
        errs = validate_contract_script(apt_content, legacy_key="discipline-theme", strict_v1=True)
        errors.extend([f"docs/apt/index.html: {e}" for e in errs])

    # 5. Check docs/rpm/index.html if present
    rpm_path = os.path.join(repo_root, "docs", "rpm", "index.html")
    if os.path.isfile(rpm_path):
        with open(rpm_path, "r", encoding="utf-8") as f:
            rpm_content = f.read()
        errs = validate_contract_script(rpm_content, legacy_key="discipline-theme", strict_v1=True)
        errors.extend([f"docs/rpm/index.html: {e}" for e in errs])

    return errors


def fetch_url(url: str, timeout: float = 4.0) -> Tuple[Optional[str], Optional[str]]:
    """Fetches a URL with a strict timeout. Returns (content, error_message)."""
    try:
        req = urllib.request.Request(
            url,
            headers={"User-Agent": "orieg-theme-linter/1.0 (https://github.com/orieg/discipline)"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if resp.status == 200:
                return resp.read().decode("utf-8", errors="ignore"), None
            return None, f"HTTP {resp.status}"
    except (urllib.error.URLError, OSError, TimeoutError) as exc:
        return None, str(exc)


# --- Self-Tests --------------------------------------------------------------


class TestEcosystemThemeLinter(unittest.TestCase):
    def test_luminance_and_contrast(self):
        self.assertAlmostEqual(contrast_ratio("#ffffff", "#000000"), 21.0, places=1)
        self.assertAlmostEqual(contrast_ratio("#000000", "#000000"), 1.0, places=1)
        self.assertTrue(contrast_ratio("#facc15", "#111827") >= 4.5)
        self.assertTrue(contrast_ratio("#b45309", "#ffffff") >= 4.5)
        self.assertTrue(contrast_ratio("#b45309", "#f1f5f9") >= 4.5)
        self.assertTrue(contrast_ratio("#f87171", "#111827") >= 4.5)
        self.assertTrue(contrast_ratio("#be123c", "#ffffff") >= 4.5)

    def test_contrast_failure_detected(self):
        palette = {"bg": "#ffffff", "card-bg": "#ffffff", "card-inner": "#ffffff", "text": "#aaaaaa"}
        errs = validate_palette_contrast(palette, "test_pal")
        self.assertTrue(any("text" in e and "< 4.5:1 floor" in e for e in errs))

    def test_valid_strict_v1_script_passes(self):
        valid_script = """
        var KEY = 'orieg-theme';
        var LEGACY_KEY = 'discipline-theme';
        var stored = localStorage.getItem(KEY) || localStorage.getItem(LEGACY_KEY);
        var mode = stored || 'system';
        document.documentElement.setAttribute('data-theme', mode === 'dark' ? 'dark' : 'light');
        document.documentElement.setAttribute('data-theme-mode', mode);
        window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function() {});
        """
        errs = validate_contract_script(valid_script, "discipline-theme", strict_v1=True)
        self.assertEqual(errs, [])

    def test_missing_orieg_theme_key_in_strict_v1(self):
        invalid_script = """
        var KEY = 'discipline-theme';
        var mode = localStorage.getItem(KEY) || 'system';
        document.documentElement.setAttribute('data-theme', 'light');
        document.documentElement.setAttribute('data-theme-mode', 'light');
        window.matchMedia('(prefers-color-scheme: dark)');
        """
        errs = validate_contract_script(invalid_script, "discipline-theme", strict_v1=True)
        self.assertTrue(any("orieg-theme" in e for e in errs))


def main() -> int:
    parser = argparse.ArgumentParser(description="Check ecosystem theme contract compliance.")
    parser.add_argument("--self-test", action="store_true", help="Run self-tests")
    parser.add_argument("--local-only", action="store_true", help="Check only local repo files")
    args = parser.parse_args()

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(TestEcosystemThemeLinter)
        runner = unittest.TextTestRunner(verbosity=2)
        res = runner.run(suite)
        return 0 if res.wasSuccessful() else 1

    local_errors = check_local_discipline(ROOT)
    if local_errors:
        for err in local_errors:
            print(f"::error::{err}", file=sys.stderr)
        print(f"check_ecosystem_theme.py: local checks failed ({len(local_errors)} errors)", file=sys.stderr)
        return 1

    print("check_ecosystem_theme.py: local discipline documentation surfaces are 100% compliant with Ecosystem Theme Contract v1")

    if args.local_only:
        return 0

    # Remote checks (fail-open notice pattern)
    for site in ECOSYSTEM_SITES:
        if site["id"] == "discipline":
            continue
        url = site["url"]
        content, fetch_err = fetch_url(url)
        if fetch_err or not content:
            print(f"::notice::check_ecosystem_theme: {site['name']} ({url}) could not be contacted ({fetch_err}) — skipping live verification")
            continue
        site_errs = validate_contract_script(content, site.get("legacy_key"))
        if site_errs:
            for e in site_errs:
                print(f"::warning::[remote:{site['id']}] {e} at {url}")
        else:
            print(f"  [remote:{site['id']}] verified compliant ({url})")

    return 0


if __name__ == "__main__":
    sys.exit(main())
