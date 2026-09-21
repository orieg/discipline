# typed: false
# frozen_string_literal: true

class Discipline < Formula
  desc "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
  homepage "https://github.com/orieg/discipline"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.5.1/discipline-aarch64-apple-darwin.tar.gz"
      sha256 "b681786b860aa68b316febe140be88ec9dcd9c4a2710745e09465d5218d794b1"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.5.1/discipline-x86_64-apple-darwin.tar.gz"
      sha256 "dabfa45fbf897d67ef51eae2e0fd70db3ad4db6d5258f519e00d0d9bacdaa385"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.5.1/discipline-aarch64-unknown-linux-musl.tar.gz"
      sha256 "63fec1f60eb4940132341010e9ae708626dee727cc9eed24f576451ab2f2bfc8"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.5.1/discipline-x86_64-unknown-linux-musl.tar.gz"
      sha256 "9b3d9f0748e4d408a1f24fb645ca87bf34f26710d4ad257ef360947421529b22"
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
