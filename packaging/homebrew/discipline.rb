# typed: false
# frozen_string_literal: true

class Discipline < Formula
  desc "Universal CI/CD gatekeeper and AI coding agent diff sentinel"
  homepage "https://github.com/orieg/discipline"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.2.2/discipline-aarch64-apple-darwin.tar.gz"
      sha256 "25b0cc61ba73b39991d46a9f7e59cbc5070d5176c1b478b748bfccb36ff6dd04"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.2.2/discipline-x86_64-apple-darwin.tar.gz"
      sha256 "bbaa36391110c0b932ac074f830bb260b4b1bca0ccdb5c26720aa2c74cafd65c"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/orieg/discipline/releases/download/v0.2.2/discipline-aarch64-unknown-linux-musl.tar.gz"
      sha256 "c6f7f1dad10fba8d762cae223cffb9975276b888579d547a7f295df34028360e"
    else
      url "https://github.com/orieg/discipline/releases/download/v0.2.2/discipline-x86_64-unknown-linux-musl.tar.gz"
      sha256 "2de9e5bfe73ed775634ec109c100af21cafe97402397b54fe94072f3e99f24d2"
    end
  end

  def install
    bin.install "discipline"
  end

  test do
    assert_match "discipline #{version}", shell_output("#{bin}/discipline --version")
  end
end
