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

# The first admin (docs/identityd.md §6), as a person would: once, as the
# register's owner. The generated password is printed once; this test keeps
# it (root-only) so the second boot can sign in with it.
runuser -u deltos-identity -- /usr/lib/deltos/bin/deltos-identity-provision \
  --display-name "VM Admin" --sign-in-name admin > /root/admin.txt
chmod 0600 /root/admin.txt
sed -n 's/^Password, shown this once and never stored: //p' /root/admin.txt > /root/admin-pw
grep -q . /root/admin-pw && echo "admin_provisioned=yes" >> $OUT
if runuser -u deltos-identity -- /usr/lib/deltos/bin/deltos-identity-provision \
     --display-name "Again" --sign-in-name admin2 >/dev/null 2>&1; then
  echo "admin_reprovision_refused=no" >> $OUT
else
  echo "admin_reprovision_refused=yes" >> $OUT
fi

# The box's certificates are not made here: deltos-tls.service makes the
# box's own CA and both server certificates at the next boot, as on a real
# box (docs/box-certificates.md), and the virtual phone fetches the CA from
# the box the way a teacher's laptop would.

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
