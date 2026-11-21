#!/bin/sh
# First boot of the test machine (run by cloud-init): install the box as a
# person would, check an unprovisioned box starts nothing and fails nothing,
# provision it, and reboot so the second boot is a real boot.
set -eux
S=/mnt/share
OUT=$S/results/setup.txt
mkdir -p $S/results
exec >"$S/results/setup.log" 2>&1

dpkg -i $S/debs/*.deb
if [ -d $S/modules ]; then
  cp -a $S/modules/. /lib/modules/
  depmod -a "$(uname -r)"
fi

# Two virtual Wi-Fi radios at every boot: wlan0 is the box's, wlan1 the phone's.
echo mac80211_hwsim > /etc/modules-load.d/deltos-vm-test.conf
echo "options mac80211_hwsim radios=2" > /etc/modprobe.d/deltos-vm-test.conf
modprobe mac80211_hwsim radios=2

$S/system/install.sh --bin-dir $S/bin

# An unprovisioned box: nothing starts, nothing fails.
systemctl start deltos-box.target
sleep 5
{
  echo "unprovisioned_failed_units=$(systemctl list-units --failed --plain --no-legend 'deltos-*' | wc -l)"
  echo "unprovisioned_active=$(systemctl list-units --state=active --plain --no-legend 'deltos-*.service' | wc -l)"
} > $OUT
systemctl stop deltos-box.target

# Provision: the settings file a person setting up the box writes. Test values.
cat > /root/network.conf <<'CONF'
mode=hotspot
interface=wlan0
address=10.42.0.1/24
uplink_interface=eth0
ssid=DeltOS VM Test
passphrase=test-only passphrase
country=GB
channel=6
CONF
/usr/lib/deltos/bin/deltos-net-provision /var/lib/deltos/network/registry.sqlite /root/network.conf >> $OUT
# a second run is refused: after the first, only checked actions change it
if /usr/lib/deltos/bin/deltos-net-provision /var/lib/deltos/network/registry.sqlite /root/network.conf; then
  echo "reprovision_refused=no" >> $OUT
else
  echo "reprovision_refused=yes" >> $OUT
fi
rm /root/network.conf

# deltd's identity source. TEST VALUE ONLY: identityd is not built, so this
# is deltd's development table. The WebOS hostname is the unit's own.
install -d -m 0755 /etc/deltos-vm-test
cat > /etc/deltos-vm-test/sessions.json <<'JSON'
{ "sessions": [ { "token": "vm-test-token", "profile_id": "learner-1", "browser_id": "phone-1",
                  "permissions": ["content.read"], "partition": "0123456789abcdef" } ] }
JSON
chmod 0644 /etc/deltos-vm-test/sessions.json
install -d /etc/systemd/system/deltos-deltd.service.d
cat > /etc/systemd/system/deltos-deltd.service.d/vm-test.conf <<'DROPIN'
[Service]
Environment=DELTD_IDENTITY_ARGS=--dev-sessions=/etc/deltos-vm-test/sessions.json
DROPIN

install -m 0644 $S/science.wax /var/lib/deltos/packs/science.wax

# The second boot runs the checks, then powers off.
cat > /etc/systemd/system/deltos-vm-test.service <<UNIT
[Unit]
Description=DeltOS VM test (second boot)
After=deltos-box.target multi-user.target
[Service]
Type=oneshot
ExecStart=$S/system/tests/vm/guest-test.sh
ExecStopPost=/bin/systemctl poweroff --no-block
StandardOutput=journal+console
TimeoutStartSec=30min
[Install]
WantedBy=multi-user.target
UNIT
systemctl enable deltos-vm-test.service
echo setup_done=yes >> $OUT
sync
systemctl reboot --no-block
