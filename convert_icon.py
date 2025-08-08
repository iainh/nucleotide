#!/usr/bin/env python3
"""Convert SVG to PNG files at multiple resolutions for macOS iconset"""

import subprocess
import os

# Define sizes needed for macOS iconset
sizes = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]

print("Note: This script requires librsvg (rsvg-convert) to be installed.")
print("You can install it with: brew install librsvg")
print()
print("Alternatively, you can use an online converter or image editing software")
print("to create PNG files at these sizes:")
print()

for size, filename in sizes:
    print(f"- {filename}: {size}x{size} pixels")

print()
print("Place the PNG files in assets/helix-gpui.iconset/")
print("Then run: iconutil -c icns assets/helix-gpui.iconset -o assets/helix-gpui.icns")