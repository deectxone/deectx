class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectxone/deectx"
  version "0.2.3"
  license "Apache-2.0"

  # Installs the prebuilt release binary — no Rust toolchain or C/C++ linker
  # needed. URLs and hashes point at the per-target archives attached to the
  # GitHub release; the release workflow refreshes `version` and the three
  # sha256 values automatically (see .github/workflows/release.yml).
  on_macos do
    on_arm do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-aarch64-apple-darwin.tar.gz"
      sha256 "ade59f6dcd3a6e2ce9e4d3e8285e227ca4c991561d0ce44a56e82b6e12a60958"
    end
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-apple-darwin.tar.gz"
      sha256 "c2d453afa54e0a6bd3202f31b6749163e8072db8b2bef8a6f27b7143b1795d70"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8ec89e3d7702a2027ea6e8d382f732ca433e19ce29eb47a5077ccb8e8d6b1e36"
    end
  end

  def install
    bin.install "deectx"
  end

  test do
    assert_match "deectx", shell_output("#{bin}/deectx --help")
  end
end
