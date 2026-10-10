#!/usr/bin/env bash
# Builds the two ipk packages without the OpenWrt SDK:
#   openwrt-clash-lite_<version>_<opkg arch>.ipk   daemon, procd service, ubus plugin, Yacd
#   luci-app-openwrt-clash-lite_<version>_all.ipk   the LuCI page
# /etc/config/openwrt-clash-lite is created by postinst, not shipped: no conffile means no
# "-opkg" copies on upgrades, and the subscriptions survive a reinstall.
# usage: scripts/build-ipk.sh <version> <cpu: armv7|aarch64> <binary> <out-dir>
# One static binary per CPU is packaged once per opkg architecture name, because opkg only
# installs a package whose Architecture is in `opkg print-architecture` (a failing preinst does
# not stop it). armv7 names are limited to targets with hardware floating point (the binary is
# armhf); soft-float ones like arm_cortex-a9 (bcm53xx) would crash.
set -euo pipefail

VERSION=$1 CPU=$2 BINARY=$3 OUT=$4
ROOT=$(cd "$(dirname "$0")/.." && pwd)
case $CPU in
	armv7)
		UNAME=armv7l
		ARCHS="arm_cortex-a5_vfpv4 arm_cortex-a7_neon-vfpv4 arm_cortex-a7_vfpv4 arm_cortex-a8_vfpv3
			arm_cortex-a9_neon arm_cortex-a9_vfpv3-d16 arm_cortex-a15_neon-vfpv4" ;;
	aarch64)
		UNAME=aarch64
		ARCHS="aarch64_generic aarch64_cortex-a53 aarch64_cortex-a72 aarch64_cortex-a76" ;;
	*) echo "unknown cpu $CPU" >&2; exit 1 ;;
esac
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

# tar with root ownership and stable order, as opkg expects
pack() { tar --owner=0 --group=0 --numeric-owner --sort=name -czf "$1" -C "$2" .; }

# $1 package dir (with data/ and control/), $2 output file
ipk() {
	echo "2.0" > "$1/debian-binary"
	pack "$1/data.tar.gz" "$1/data"
	pack "$1/control.tar.gz" "$1/control"
	tar --owner=0 --group=0 --numeric-owner -czf "$2" -C "$1" ./debian-binary ./data.tar.gz ./control.tar.gz
	echo "built $2"
}

control() { # $1 dir, $2 package, $3 arch, $4 depends, $5 description
	local size
	size=$(du -sk "$1/data" | cut -f1)
	mkdir -p "$1/control"
	cat > "$1/control/control" <<EOF
Package: $2
Version: $VERSION
Depends: $4
Source: https://github.com/x1t/openwrt-clash-lite
License: MIT
Section: net
Architecture: $3
Installed-Size: $((size * 1024))
Maintainer: x1t
Description: $5
EOF
}

core_package() {
	local d=$WORK/core
	install -Dm755 "$BINARY" "$d/data/usr/bin/openwrt-clash-lite"
	install -Dm755 "$ROOT/files/openwrt-clash-lite.init" "$d/data/etc/init.d/openwrt-clash-lite"
	install -Dm644 "$ROOT/files/oclite.uc" "$d/data/usr/share/rpcd/ucode/oclite"
	mkdir -p "$d/data/usr/share/openwrt-clash-lite/ui"
	unzip -q "$ROOT/assets/yacd.zip" -d "$d/data/usr/share/openwrt-clash-lite/ui" -x CNAME _headers
	for arch in $ARCHS; do
		control "$d" openwrt-clash-lite "$arch" "kmod-tun, kmod-nft-nat, rpcd-mod-ucode, ucode-mod-fs" \
			"Lightweight mihomo transparent proxy (TUN takeover) with subscriptions"
		for s in preinst postinst prerm postrm; do
			sed "s/@UNAME@/$UNAME/" "$ROOT/files/ipk/core.$s" > "$d/control/$s"
			chmod 755 "$d/control/$s"
		done
		ipk "$d" "$OUT/openwrt-clash-lite_${VERSION}_${arch}.ipk"
	done
}

luci_package() {
	local d=$WORK/luci
	mkdir -p "$d/data"
	cp -r "$ROOT/luci/htdocs" "$d/data/www"
	cp -r "$ROOT/luci/root/." "$d/data/"
	control "$d" luci-app-openwrt-clash-lite all "luci-base, openwrt-clash-lite" "LuCI page for openwrt-clash-lite"
	for s in postinst postrm; do
		install -m755 "$ROOT/files/ipk/luci.$s" "$d/control/$s"
	done
	ipk "$d" "$OUT/luci-app-openwrt-clash-lite_${VERSION}_all.ipk"
}

core_package
luci_package
