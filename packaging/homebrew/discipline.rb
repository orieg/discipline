# typed: false
# frozen_string_literal: true

class Discipline < Formula
  desc "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
  homepage "https://github.com/orieg/discipline"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.4.0/discipline-aarch64-apple-darwin.tar.gz"
      sha256 "5cc39691aa744aa8d983579f6f210b16851749034295c83bcf6ddcbdeced2fbf"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.4.0/discipline-x86_64-apple-darwin.tar.gz"
      sha256 "f9db5bef3b9832d0ac4ceeb44fdf96f8a7fee28002cf5fa904b0ea84e83c4526"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.4.0/discipline-aarch64-unknown-linux-musl.tar.gz"
      sha256 "c4c2fd762f6fc01d154918f577c8c07a1307efa5164d3ebac2aa48896bff5415"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.4.0/discipline-x86_64-unknown-linux-musl.tar.gz"
      sha256 "f042dbc8fe688fe11189ee2eb98853a99bfd69e74d17e250e852ecdc259d410d"
    end
  end

  def install
    bin.install "discipline"
  end

  test do
    assert_match "discipline #{version}", shell_output("#{bin}/discipline --version")
  end
end
