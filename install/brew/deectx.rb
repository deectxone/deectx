class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectxone/deectx"
  version "0.2.0"
  license "Apache-2.0"

  # Installs the prebuilt release binary — no Rust toolchain or C/C++ linker
  # needed. URLs and hashes point at the per-target archives attached to the
  # GitHub release; the release workflow refreshes `version` and the three
  # sha256 values automatically (see .github/workflows/release.yml).
  on_macos do
    on_arm do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-aarch64-apple-darwin.tar.gz"
      sha256 "dcc6e3e93059ec78887956cedd7f5ab8df24dd272497fca78293f06de3ca5a6b"
    end
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-apple-darwin.tar.gz"
      sha256 "51237aa57c4a7c110f6e3df332e64b23fcd526694afb8e18e565b6e8d18016aa"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d4a70eabe00f587771c01a616cdf51e8defaf196048abd5a3f55981159355390"
    end
  end

  def install
    bin.install "deectx"
  end

  test do
    assert_match "deectx", shell_output("#{bin}/deectx --help")
  end
end
