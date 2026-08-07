class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectxone/deectx"
  version "0.2.1"
  license "Apache-2.0"

  # Installs the prebuilt release binary — no Rust toolchain or C/C++ linker
  # needed. URLs and hashes point at the per-target archives attached to the
  # GitHub release; the release workflow refreshes `version` and the three
  # sha256 values automatically (see .github/workflows/release.yml).
  on_macos do
    on_arm do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-aarch64-apple-darwin.tar.gz"
      sha256 "c4c9966e9ed63c3dc9fc2b23453ed4ef17bf300862257a20a0335b122a622573"
    end
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-apple-darwin.tar.gz"
      sha256 "f73cc04d34ad3c3bc8bb3451780dd9e0b1b990f876ebff86dd569aefa4d2900a"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "44fcc45b1de240d94eda69228c1ebae132779a3f822fad04fe0fa3789dc821b2"
    end
  end

  def install
    bin.install "deectx"
  end

  test do
    assert_match "deectx", shell_output("#{bin}/deectx --help")
  end
end
