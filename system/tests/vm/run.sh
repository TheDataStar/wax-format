#!/bin/sh
# Boot an emulated ARM64 machine (the minimum spec: aarch64, 2 GB RAM) with
# two virtual Wi-Fi radios, install the box on it, reboot, and check from a
# virtual phone that everything came up on its own. No real Pi, no real
# radio (Commander's hold on hardware testing, 2026-11-20).
#
#   system/tests/vm/run.sh --image IMG --debs DIR --bin-dir DIR --pack FILE [--work DIR]
#
# IMG      Ubuntu 24.04 arm64 cloud image (ubuntu-24.04-server-cloudimg-arm64.img)
# --debs   arm64 .debs the image lacks: linux-modules-extra-<its kernel>
#          (mac80211_hwsim), hostapd, dnsmasq-base, iw, wpasupplicant,
#          libpcsclite1, wireless-regdb
# --bin-dir  aarch64 builds of deltd, deltos-net-render, deltos-net-provision,
#          deltos-identityd, deltos-identity-provision, deltos-tls-provision,
#          deltos-backup and the example
#          configure_ap_standin
# --pack   any .wax pack holding a page that matches "evaporation"
#
# Needs qemu-system-aarch64, AAVMF/QEMU_EFI firmware, qemu-img,
# cloud-localds and mkfs.ext4/debugfs. Without KVM it runs under TCG
# emulation: expect 15 to 40 minutes. Exits 0 only on "RESULT PASS".
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SYSTEM=$(cd "$HERE/../.." && pwd)
IMG=""; DEBS=""; BIN=""; PACK=""; WORK=""
while [ $# -gt 0 ]; do
    case "$1" in
    --image) IMG=$2; shift 2 ;;
    --debs) DEBS=$2; shift 2 ;;
    --bin-dir) BIN=$2; shift 2 ;;
    --pack) PACK=$2; shift 2 ;;
    --work) WORK=$2; shift 2 ;;
    *) echo "run.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMG" ] && [ -n "$DEBS" ] && [ -n "$BIN" ] && [ -n "$PACK" ] || { sed -n '2,20p' "$0"; exit 2; }
WORK=${WORK:-$(mktemp -d)}
mkdir -p "$WORK"
echo "work dir: $WORK"

# the shared disk: everything the guest needs, and where results come back
SH=$WORK/share
rm -rf "$SH"; mkdir -p "$SH/bin" "$SH/debs" "$SH/results" "$SH/system"
cp -a "$SYSTEM/." "$SH/system/"
for b in deltd deltos-net-render deltos-net-provision deltos-identityd deltos-identity-provision deltos-tls-provision deltos-backup; do cp "$BIN/$b" "$SH/bin/"; done
cp "$BIN/examples/configure_ap_standin" "$SH/bin/" 2>/dev/null || cp "$BIN/configure_ap_standin" "$SH/bin/"
# The virtual radio's kernel modules come from linux-modules-extra. Only the
# Wi-Fi ones are copied in: installing the whole package rebuilds the boot
# image, which takes most of an hour under emulation.
for d in "$DEBS"/*.deb; do
    case "$(basename "$d")" in
    linux-modules-extra-*)
        x=$WORK/mx; rm -rf "$x"; mkdir -p "$x"; dpkg-deb -x "$d" "$x"
        for k in "$x"/lib/modules/*; do
            v=$(basename "$k")
            for sub in net/mac80211 net/wireless drivers/net/wireless/virtual; do
                mkdir -p "$SH/modules/$v/kernel/$sub"
                cp -a "$k/kernel/$sub/." "$SH/modules/$v/kernel/$sub/"
            done
        done ;;
    *) cp "$d" "$SH/debs/" ;;
    esac
done
cp "$PACK" "$SH/science.wax"
rm -f "$WORK/share.img"
mkfs.ext4 -q -L DELTSHARE -d "$SH" "$WORK/share.img" 1G

cat > "$WORK/user-data" <<'UD'
#cloud-config
mounts:
  - [ "LABEL=DELTSHARE", /mnt/share, ext4, "defaults,nofail", "0", "2" ]
runcmd:
  - [ sh, -c, "mkdir -p /mnt/share && (mountpoint -q /mnt/share || mount LABEL=DELTSHARE /mnt/share) && /mnt/share/system/tests/vm/guest-setup.sh" ]
UD
printf 'instance-id: deltos-vm-test\nlocal-hostname: deltos-box\n' > "$WORK/meta-data"
cloud-localds "$WORK/seed.img" "$WORK/user-data" "$WORK/meta-data"

qemu-img create -q -f qcow2 -F qcow2 -b "$(realpath "$IMG")" "$WORK/disk.qcow2" 8G
FW=/usr/share/AAVMF/AAVMF_CODE.fd
[ -e "$FW" ] || FW=/usr/share/qemu-efi-aarch64/QEMU_EFI.fd
cp /usr/share/AAVMF/AAVMF_VARS.fd "$WORK/vars.fd" 2>/dev/null || truncate -s 64M "$WORK/vars.fd"
ACCEL="-accel tcg,thread=multi -cpu max"
[ -w /dev/kvm ] && [ "$(uname -m)" = aarch64 ] && ACCEL="-accel kvm -cpu host"

# Runs both boots; the guest powers itself off after the checks.
timeout 90m qemu-system-aarch64 -M virt $ACCEL -smp 4 -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file="$FW" \
    -drive if=pflash,format=raw,file="$WORK/vars.fd" \
    -drive if=virtio,format=qcow2,file="$WORK/disk.qcow2" \
    -drive if=virtio,format=raw,file="$WORK/seed.img" \
    -drive if=virtio,format=raw,file="$WORK/share.img" \
    -nic user,model=virtio-net-pci \
    -display none -serial file:"$WORK/console.log" 2>"$WORK/qemu.log" \
    || { echo "qemu ended abnormally (see $WORK/qemu.log, $WORK/console.log)"; }

for f in setup.txt setup.log test.txt journal.txt; do
    debugfs -R "dump /results/$f $WORK/$f" "$WORK/share.img" >/dev/null 2>&1 || true
done
echo "--- first boot (install, unprovisioned check, provision) ---"
cat "$WORK/setup.txt" 2>/dev/null || echo "no setup results; see $WORK/setup.log and $WORK/console.log"
echo "--- second boot (the box came up on its own) ---"
cat "$WORK/test.txt" 2>/dev/null || echo "no test results; see $WORK/console.log"
grep -q '^unprovisioned_failed_units=0$' "$WORK/setup.txt" 2>/dev/null || { echo "FAIL: an unprovisioned box had failed units"; exit 1; }
grep -q '^unprovisioned_active=0$' "$WORK/setup.txt" 2>/dev/null || { echo "FAIL: an unprovisioned box started services"; exit 1; }
grep -q '^reprovision_refused=yes$' "$WORK/setup.txt" 2>/dev/null || { echo "FAIL: provisioning ran twice"; exit 1; }
grep -q '^admin_reprovision_refused=yes$' "$WORK/setup.txt" 2>/dev/null || { echo "FAIL: a second first admin was made"; exit 1; }
grep -q '^RESULT PASS$' "$WORK/test.txt" 2>/dev/null
