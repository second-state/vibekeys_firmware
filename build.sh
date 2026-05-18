#!/bin/bash

# Build script for vibekeys project
# Usage: ./build.sh [ota|keys]

MODE="${1:-}"

case "$MODE" in
    ota)
        echo "Building OTA image..."
        cargo build --bin ota --release
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/ota ./ota.bin
        ;;
    keys)
        echo "Building keys image..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_ota.bin
        ;;
    max2)
        echo "Building max2 image..."
        cargo build --bin vibekeys --release --features max2
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2_ota.bin
        ;;
    keys_bin)
        echo "Building keys binary image..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys.bin
        ;;
    max2_bin)
        echo "Building max2 binary image..."
        cargo build --bin vibekeys --release --features max2
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2.bin
        ;;
    keys_ota_bin)
        echo "Building keys binary image with OTA header..."
        cargo build --bin vibekeys --release
        cargo build --bin ota --release
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys.bin
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/ota ./ota.bin
        dd if=ota.bin of=vibekeys.bin bs=1 seek=$((0x210000)) conv=notrunc
        ;;
    max2_ota_bin)
        echo "Building max2 binary image with OTA header..."
        cargo build --bin vibekeys --release --features max2
        cargo build --bin ota --release --features max2
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2.bin
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/ota ./ota.bin
        dd if=ota.bin of=vibekeys_max2.bin bs=1 seek=$((0x210000)) conv=notrunc
        ;;
    *)
        echo "Usage: $0 {ota|keys|max2|keys_bin|max2_bin|keys_ota_bin|max2_ota_bin}"
        echo ""
        echo "  ota   - Build OTA image"
        echo "  keys  - Build keys image"
        echo "  max2  - Build max2 image"
        echo "  keys_bin - Build keys binary image (without OTA header)"
        echo "  max2_bin - Build max2 binary image (without OTA header)"
        echo "  keys_ota_bin - Build keys binary image with OTA header"
        echo "  max2_ota_bin - Build max2 binary image with OTA header"
        exit 1
        ;;
esac
