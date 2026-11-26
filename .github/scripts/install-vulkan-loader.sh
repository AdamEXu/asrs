#!/usr/bin/env bash
set -euo pipefail

version=${VULKAN_LOADER_VERSION:-1.4.345}
case "$version" in
  1.4.345)
    loader_sha256=a6b62ea8e51053a8622be2fa293e2bae658dbbab0e292214c358e4b8fed8064a
    headers_sha256=99e19205df172c33229f18b5574c9258bc73ea229ec7d6b04daa357b2430f6c8
    ;;
  *)
    echo "no pinned checksums for Vulkan-Loader $version" >&2
    exit 1
    ;;
esac

prefix=${1:?usage: install-vulkan-loader.sh <prefix>}
if [[ -f "$prefix/lib/libvulkan.so.$version" &&
  -L "$prefix/lib/libvulkan.so.1" &&
  -L "$prefix/lib/libvulkan.so" ]]; then
  exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$prefix"
cd "$work"

download() {
  local url=$1 file=$2 sha256=$3
  curl -fL --retry 3 --retry-delay 2 -o "$file" "$url"
  printf '%s  %s\n' "$sha256" "$file" | sha256sum -c -
}

download \
  "https://github.com/KhronosGroup/Vulkan-Headers/archive/refs/tags/v$version.tar.gz" \
  headers.tar.gz \
  "$headers_sha256"
download \
  "https://github.com/KhronosGroup/Vulkan-Loader/archive/refs/tags/v$version.tar.gz" \
  loader.tar.gz \
  "$loader_sha256"

tar -xzf headers.tar.gz
tar -xzf loader.tar.gz

headers_prefix="$work/headers-prefix"
cmake -S "Vulkan-Headers-$version" -B build-headers \
  -DCMAKE_INSTALL_PREFIX="$headers_prefix" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DVULKAN_HEADERS_ENABLE_TESTS=OFF \
  -DVULKAN_HEADERS_ENABLE_MODULE=OFF
cmake --build build-headers --target install

cmake -S "Vulkan-Loader-$version" -B build-loader \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DVULKAN_HEADERS_INSTALL_DIR="$headers_prefix" \
  -DBUILD_TESTS=OFF
cmake --build build-loader --parallel "$(nproc)"
cmake --install build-loader

test -f "$prefix/lib/libvulkan.so.$version"
