# typed: false
# frozen_string_literal: true

class Discipline < Formula
  desc "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
  homepage "https://github.com/orieg/discipline"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.6.0/discipline-aarch64-apple-darwin.tar.gz"
      sha256 "883d78fca0d7cc83100d67574263f3f3a7df9a8dc714eb0e0f371932372bb488"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.6.0/discipline-x86_64-apple-darwin.tar.gz"
      sha256 "9696125cfb6b460bd0cd9309129f39dfa06bfd368003392f5a46fa008fe7da03"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.6.0/discipline-aarch64-unknown-linux-musl.tar.gz"
      sha256 "f069f16e6606ccb29926e7f235ae3208ea760a0e5376d9222e2c6db6a61829be"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.6.0/discipline-x86_64-unknown-linux-musl.tar.gz"
      sha256 "e1cf14f0de9cb5a5a0b8c4d24e8829465e6ab6a885f74c8b326b037f818beb44"
    end
  end

  def install
    bin.install "discipline"
    man1.install "man/man1/discipline.1" if File.exist?("man/man1/discipline.1")
    man5.install "man/man5/discipline.toml.5" if File.exist?("man/man5/discipline.toml.5")
  end

  test do
    assert_match "discipline #{version}", shell_output("#{bin}/discipline --version")
  end
end
