class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectx/deectx"
  url "https://crates.io/api/v1/crates/deectx/0.1.0/download"
  sha256 "fill-from-release"
  license "Apache-2.0"

  depends_on "rust" => :build

  # Installs the crate from crates.io via `cargo install deectx@0.1.0`.
  def install
    system "cargo", "install", "deectx@0.1.0", "--root", prefix
  end

  test do
    assert_match "deectx", shell_output("#{bin}/deectx --help")
  end
end