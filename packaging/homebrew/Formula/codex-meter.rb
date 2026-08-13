class CodexMeter < Formula
  desc "Local-first Codex usage, cost, cache, and performance dashboard"
  homepage "https://github.com/DelicateNorman/codex-meter"
  version "0.16.1"
  license "MIT"

  depends_on macos: :monterey

  on_arm do
    resource "binary" do
      url "https://github.com/DelicateNorman/codex-meter/releases/download/v0.16.1/codex-meter-macos-arm64",
          using: :nounzip
      sha256 "f0efc4ce28269b32881b6cb4d1c4da1fd32d88a5f2705118c450b5308b5ad5cc"
    end
  end

  on_intel do
    resource "binary" do
      url "https://github.com/DelicateNorman/codex-meter/releases/download/v0.16.1/codex-meter-macos-x86_64",
          using: :nounzip
      sha256 "fdfd070ac3e8f9da4c3a6c84e72f8c13b527415492e5617cb853277f2bd2459e"
    end
  end

  def install
    asset = Hardware::CPU.arm? ? "codex-meter-macos-arm64" : "codex-meter-macos-x86_64"
    resource("binary").stage do
      bin.install asset => "codex-meter"
    end
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/codex-meter --version")
  end
end
