#!/bin/bash

# Build script for vibekeys project (单二进制,A/B OTA)
# Usage: ./build.sh {keys|max2|keys_bin|max2_bin|keys_ota_bin|max2_ota_bin}
#
# keys / max2                 - OTA 镜像(仅 app),供 OTA 上传 / download-latest
# keys_bin / max2_bin         - 合并工厂镜像,app 在 ota_1
# keys_ota_bin / max2_ota_bin - 合并工厂镜像,app 在 ota_0(首启槽;OTA 会写 ota_1)

MODE="${1:-}"

case "$MODE" in
    keys)
        echo "Building keys OTA image..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_ota.bin
        ;;
    max2)
        echo "Building max2 OTA image..."
        cargo build --bin vibekeys --release --features max2
        espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2_ota.bin
        ;;
    keys_bin)
        echo "Building keys factory image (ota_1)..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys.bin
        ;;
    max2_bin)
        echo "Building max2 factory image (ota_1)..."
        cargo build --bin vibekeys --release --features max2
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_1 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2.bin
        ;;
    keys_ota_bin)
        echo "Building keys factory image (ota_0, first boot slot)..."
        cargo build --bin vibekeys --release
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys.bin
        ;;
    max2_ota_bin)
        echo "Building max2 factory image (ota_0, first boot slot)..."
        cargo build --bin vibekeys --release --features max2
        espflash save-image --chip esp32s3 --merge --flash-size 16mb --partition-table partitions.csv --target-app-partition ota_0 target/xtensa-esp32s3-espidf/release/vibekeys ./vibekeys_max2.bin
        ;;
    *)
        echo "Usage: $0 {keys|max2|keys_bin|max2_bin|keys_ota_bin|max2_ota_bin}"
        echo ""
        echo "  keys / max2                 - OTA image for OTA upload / download-latest"
        echo "  keys_bin / max2_bin         - merged factory image, app in ota_1"
        echo "  keys_ota_bin / max2_ota_bin - merged factory image, app in ota_0 (first boot)"
        exit 1
        ;;
esac
