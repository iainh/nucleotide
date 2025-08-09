#!/bin/bash
# Create PNG files for macOS iconset using rsvg-convert from nix

# Define sizes needed for macOS iconset
sizes=(
    "16:icon_16x16.png"
    "32:icon_16x16@2x.png"
    "32:icon_32x32.png"
    "64:icon_32x32@2x.png"
    "128:icon_128x128.png"
    "256:icon_128x128@2x.png"
    "256:icon_256x256.png"
    "512:icon_256x256@2x.png"
    "512:icon_512x512.png"
    "1024:icon_512x512@2x.png"
)

echo "Converting SVG to PNG files..."

for item in "${sizes[@]}"; do
    size="${item%%:*}"
    filename="${item##*:}"
    echo "Creating $filename ($size x $size)"
    nix-shell -p librsvg --run "rsvg-convert -w $size -h $size assets/logo.svg -o assets/nucleotide.iconset/$filename"
done

echo "Creating ICNS file..."
iconutil -c icns assets/nucleotide.iconset -o assets/nucleotide.icns

echo "Done! Icon created at assets/nucleotide.icns"