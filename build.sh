#!/bin/bash

# Build script for vibekeys project
# Usage: ./build.sh [ota|keys]

MODE="${1:-}"

case "$MODE" in
    ota)
        echo "Building OTA image..."
        cargo build --bin ota --release
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/ota ./ota.bin
        ;;
    keys)
        echo "Building keys image..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys.bin
        ;;
    *)
        echo "Usage: $0 {ota|keys}"
        echo ""
        echo "  ota   - Build OTA image"
        echo "  keys  - Build keys image"
        exit 1
        ;;
esac
