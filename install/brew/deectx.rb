class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectxone/deectx"
  url "https://crates.io/api/v1/crates/deectx/0.1.0/download"
  sha256 "47f4fd30de1f9773f1b8a07968c27c645879bb1bf4435165f5d912cb13df55c1"
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