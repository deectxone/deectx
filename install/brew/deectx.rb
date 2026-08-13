class Deectx < Formula
  desc "Local-first PII-masking proxy for AI coding tools"
  homepage "https://github.com/deectxone/deectx"
  version "0.2.5"
  license "Apache-2.0"

  # Installs the prebuilt release binary — no Rust toolchain or C/C++ linker
  # needed. URLs and hashes point at the per-target archives attached to the
  # GitHub release; the release workflow refreshes `version` and the three
  # sha256 values automatically (see .github/workflows/release.yml).
  on_macos do
    on_arm do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-aarch64-apple-darwin.tar.gz"
      sha256 "9b3e0974a6e893a1379e9e231226bdee05ba950aec533ec9ec48ae50a5d5c920"
    end
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-apple-darwin.tar.gz"
      sha256 "426050464ca8aaf7547c1c725336d5366689adf0868f8b401b88cd62eb1a094b"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/deectxone/deectx/releases/download/v#{version}/deectx-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d0ceb3732f805470ce20eee8014dfe5f82bd06870a3f18a529a0a42dfcde0cc6"
    end
  end

  def install
    bin.install "deectx"
  end

  test do
    assert_match "deectx", shell_output("#{bin}/deectx --help")
  end
end
