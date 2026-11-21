#!/bin/sh
# Install the DeltOS box services (docs/box-radio-and-boot.md §5).
#
#   system/install.sh --bin-dir DIR [--destdir DIR] [--no-enable]
#
# --bin-dir   where the built deltd, deltos-net-render, deltos-net-provision,
#             deltos-identityd and deltos-identity-provision are
#             (e.g. target/release)
# --destdir   install into an image tree instead of this machine (the
#             appliance image build); nothing is enabled or started there
#             beyond the target link
# --no-enable install only; do not enable the boot target
#
# It does not start anything and does not write network settings: a new box
# gets those once, from deltos-net-provision, and the services start at the
# next boot (or with: systemctl start deltos-box.target).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BIN=""; DEST=""; ENABLE=1
while [ $# -gt 0 ]; do
    case "$1" in
    --bin-dir) BIN=$2; shift 2 ;;
    --destdir) DEST=$2; shift 2 ;;
    --no-enable) ENABLE=0; shift ;;
    *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
die() { echo "install.sh: $*" >&2; exit 1; }
[ -n "$BIN" ] || die "--bin-dir is required"
BINS="deltd deltos-net-render deltos-net-provision deltos-identityd deltos-identity-provision"
for b in $BINS; do
    [ -x "$BIN/$b" ] || die "$BIN/$b is missing; build with: cargo build --release -p deltlib -p deltos-net -p deltos-identity"
done

if [ -z "$DEST" ]; then
    [ "$(id -u)" = 0 ] || die "run as root"
    [ -d /run/systemd/system ] || die "this machine does not run systemd"
    for t in hostapd dnsmasq nft ip sha256sum; do
        command -v "$t" >/dev/null 2>&1 || [ -x "/usr/sbin/$t" ] || die "$t is missing (Debian/Ubuntu: apt install hostapd dnsmasq-base nftables iproute2)"
    done
    # Two DNS or DHCP servers on one machine fight over the same ports.
    # The person's own services are theirs: say so and stop, never disable them.
    for s in dnsmasq.service hostapd.service; do
        if systemctl is-active --quiet "$s"; then
            die "$s is already running here. DeltOS runs its own copy (deltos-${s}); stop and disable $s first if this machine is to be a DeltOS box"
        fi
    done
fi

U=$DEST/usr/lib
install -d "$U/deltos/bin" "$U/deltos/libexec" "$U/systemd/system" "$U/sysusers.d" "$U/tmpfiles.d"
for b in $BINS; do
    install -m 0755 "$BIN/$b" "$U/deltos/bin/$b"
done
for f in "$HERE"/libexec/*; do install -m 0755 "$f" "$U/deltos/libexec/"; done
for f in "$HERE"/systemd/*; do install -m 0644 "$f" "$U/systemd/system/"; done
install -m 0644 "$HERE/sysusers.d/deltos.conf" "$U/sysusers.d/deltos.conf"
install -m 0644 "$HERE/tmpfiles.d/deltos.conf" "$U/tmpfiles.d/deltos.conf"

if [ -n "$DEST" ]; then
    if [ "$ENABLE" = 1 ]; then
        install -d "$DEST/etc/systemd/system/multi-user.target.wants"
        ln -sf /usr/lib/systemd/system/deltos-box.target "$DEST/etc/systemd/system/multi-user.target.wants/deltos-box.target"
    fi
    echo "installed into $DEST"
    exit 0
fi

systemd-sysusers deltos.conf
systemd-tmpfiles --create deltos.conf
systemctl daemon-reload
if [ "$ENABLE" = 1 ]; then
    systemctl enable deltos-box.target
fi
cat <<MSG
DeltOS box services installed.
Next, once per box:
  1. network settings:  /usr/lib/deltos/bin/deltos-net-provision /var/lib/deltos/network/registry.sqlite <settings-file>
  2. the first admin:   runuser -u deltos-identity -- /usr/lib/deltos/bin/deltos-identity-provision \
                          --display-name 'Your name' --sign-in-name admin
                        (prints the generated password once; only its hash is kept)
  3. certificates:      the box's *.deltos.lan and *.deltos-packs.lan certificates in /var/lib/deltos/tls/
                        (without them deltd serves plain HTTP only, and admins cannot sign in)
  4. start:             systemctl start deltos-box.target   (or reboot)
MSG
