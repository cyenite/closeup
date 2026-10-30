#!/usr/bin/env bash
# Regenerates docs/media/demo.gif and the stills from the real renderer.
# Needs a GPU (Vulkan), Python 3 with Pillow, and ffmpeg.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$root/target/demo"
cd "$root"
python3 -I scripts/demo/assets.py "$work"
rm -rf "$work/frames"
cargo run --release --locked --example demo -- "$work"
python3 -I scripts/demo/build.py "$work" docs/media
