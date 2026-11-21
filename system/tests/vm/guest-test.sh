#!/bin/bash
# Second boot of the test machine: the box came up on its own. Join it from
# a virtual phone and check what a phone and an attacker would see.
# Writes TAP-style lines to results/test.txt and a final RESULT line.
set -u
S=/mnt/share
OUT=$S/results/test.txt
: > "$OUT"
FAILS=0
ok()   { echo "ok - $*" | tee -a "$OUT" > /dev/console; }
bad()  { echo "not ok - $*" | tee -a "$OUT" > /dev/console; FAILS=$((FAILS+1)); }
check() { local what=$1; shift; if "$@" >/dev/null 2>&1; then ok "$what"; else bad "$what"; fi; }
info() { echo "# $*" | tee -a "$OUT" > /dev/console; }
until_t() { local t=$1; shift; local i=0; while ! "$@" >/dev/null 2>&1; do i=$((i+1)); [ $i -ge "$t" ] && return 1; sleep 1; done; }
P="ip netns exec phone"
pid() { systemctl show -p MainPID --value "$1"; }

info "kernel $(uname -r) $(uname -m), RAM $(awk '/MemTotal/{print $2}' /proc/meminfo) kB"

# --- the box started itself -------------------------------------------------
for u in deltos-net-render deltos-hotspot-address deltos-uplink-firewall deltos-hostapd deltos-dnsmasq deltos-deltd; do
  if until_t 120 systemctl is-active --quiet "$u.service"; then ok "$u started at boot"; else bad "$u started at boot ($(systemctl is-active $u.service))"; journalctl -b -u $u --no-pager | tail -20 >> "$OUT"; fi
done
check "deltos-net-changed.path watching" systemctl is-active --quiet deltos-net-changed.path
check "no DeltOS unit failed" test "$(systemctl list-units --failed --plain --no-legend 'deltos-*' | wc -l)" = 0
info "boot: $(systemd-analyze 2>/dev/null | head -1)"
for u in deltos-net-render deltos-hostapd deltos-dnsmasq deltos-deltd; do
  info "$u became active $(systemctl show -p ActiveEnterTimestampMonotonic --value $u.service | awk '{printf "%.1f", $1/1e6}') s after boot"
done

# --- privilege separation ----------------------------------------------------
check "deltd runs as deltos-deltd" test "$(ps -o user= -p "$(pid deltos-deltd)")" = deltos-deltd
check "dnsmasq runs as deltos-dns" test "$(ps -o user= -p "$(pid deltos-dnsmasq)")" = deltos-dns
check "deltd holds only CAP_NET_BIND_SERVICE" grep -q '^CapEff:[[:space:]]*0000000000000400$' "/proc/$(pid deltos-deltd)/status"
check "dnsmasq holds only bind, raw and net-admin" grep -q '^CapEff:[[:space:]]*0000000000003400$' "/proc/$(pid deltos-dnsmasq)/status"
check "hostapd holds only net-admin and raw" grep -q '^CapEff:[[:space:]]*0000000000003000$' "/proc/$(pid deltos-hostapd)/status"
check "deltos-deltd is in deltos-shellc" bash -c 'id -nG deltos-deltd | grep -qw deltos-shellc'
check "hostapd.conf (Wi-Fi password) is root-only 0600" test "$(stat -c '%a %U' /run/deltos/network/hostapd.conf)" = "600 root"
check "the registry's directory is root-only 0700" test "$(stat -c '%a %U' /var/lib/deltos/network)" = "700 root"
check "deltd cannot see the registry or settings" bash -c "! nsenter -t $(pid deltos-deltd) -m test -e /var/lib/deltos/network/registry.sqlite && ! nsenter -t $(pid deltos-deltd) -m test -e /run/deltos/network/hostapd.conf"
check "dnsmasq cannot see the registry" bash -c "! nsenter -t $(pid deltos-dnsmasq) -m test -e /var/lib/deltos/network/registry.sqlite"
check "deltd cannot write the packs" bash -c "! nsenter -t $(pid deltos-deltd) -m -S 0 touch /var/lib/deltos/packs/x"
check "hostapd has no control socket" bash -c '! ls /var/run/hostapd 2>/dev/null | grep -q .'

# --- a phone joins ----------------------------------------------------------
ip netns add phone
iw phy phy1 set netns name phone
$P ip link set lo up
$P ip link set wlan1 up
mkdir -p /etc/netns/phone
echo "nameserver 10.42.0.1" > /etc/netns/phone/resolv.conf
cat > /tmp/phone.conf <<'W'
ctrl_interface=/run/wpa-phone
network={
  ssid="DeltOS VM Test"
  psk="test-only passphrase"
  key_mgmt=WPA-PSK
}
W
cat > /tmp/phone-wrong.conf <<'W'
ctrl_interface=/run/wpa-phone
network={
  ssid="DeltOS VM Test"
  psk="not the passphrase"
  key_mgmt=WPA-PSK
}
W
joined() { $P iw dev wlan1 link | grep -q 'Connected to'; }

$P wpa_supplicant -B -i wlan1 -c /tmp/phone-wrong.conf -P /run/wpa-phone.pid >/dev/null
if until_t 25 joined; then bad "a wrong Wi-Fi password is refused"; else ok "a wrong Wi-Fi password is refused"; fi
kill "$(cat /run/wpa-phone.pid)"; sleep 1

$P wpa_supplicant -B -i wlan1 -c /tmp/phone.conf -P /run/wpa-phone.pid >/dev/null
if until_t 60 joined; then ok "phone joins the hotspot with the right password"; else bad "phone joins the hotspot"; fi
info "$($P iw dev wlan1 link | grep -E 'SSID|freq' | tr '\n' ' ')"

cat > /tmp/udhcpc.sh <<'U'
#!/bin/sh
[ "$1" = bound ] || exit 0
ip addr add "$ip/$mask" dev "$interface"
ip route add default via "$router"
echo "$ip $dns $router" > /tmp/lease.txt
U
chmod +x /tmp/udhcpc.sh
rm -f /tmp/lease.txt
$P busybox udhcpc -i wlan1 -n -q -f -t 10 -T 2 -s /tmp/udhcpc.sh >/dev/null 2>&1
read -r PIP PDNS PGW < /tmp/lease.txt 2>/dev/null || true
info "phone got ${PIP:-nothing}, DNS ${PDNS:-?}, router ${PGW:-?}"
check "phone gets an address on 10.42.0.0/24" bash -c "[[ '${PIP:-}' == 10.42.0.* && '${PIP:-}' != 10.42.0.1 ]]"
check "DHCP names the box as DNS server" test "${PDNS:-}" = 10.42.0.1
check "the lease is visible to the box" grep -q "${PIP:-none}" /run/deltos/network/leases

dig1() { $P dig +short +time=2 +tries=1 @10.42.0.1 "$1" | head -1; }
check "WebOS name resolves to the box" test "$(dig1 webos-test.deltos.lan)" = 10.42.0.1
check "a fresh pack hostname resolves to the box" test "$(dig1 pack-3-g1-0123456789abcdef.deltos-packs.lan)" = 10.42.0.1
check "an outside name is 'no such name' at once (offline)" bash -c "$P dig +time=2 +tries=1 @10.42.0.1 example.com | grep -q 'status: NXDOMAIN'"
check "Android's internet check resolves to the box" test "$(dig1 connectivitycheck.gstatic.com)" = 10.42.0.1

C="$P curl -s -o /dev/null -w %{http_code} --max-time 5"
check "box API refuses a phone with no session (401)" test "$($C http://webos-test.deltos.lan/api/v1/status)" = 401
check "box API answers a signed-in phone (200)" test "$($C -H 'Cookie: delt_session=vm-test-token' http://webos-test.deltos.lan/api/v1/status)" = 200
check "search across packs finds the page" bash -c "$P curl -s --max-time 5 -H 'Cookie: delt_session=vm-test-token' 'http://webos-test.deltos.lan/api/v1/search?q=evaporation' | grep -q 'Water cycle'"
check "Android's check is answered 'internet works' (204)" test "$($C http://connectivitycheck.gstatic.com/generate_204)" = 204
check "Apple's check is answered 'Success'" bash -c "$P curl -s --max-time 5 http://captive.apple.com/hotspot-detect.html | grep -q Success"
check "a name deltd does not serve is refused" test "$($C -H 'Host: evil.example' http://10.42.0.1/)" = 404
check "nothing from the hotspot is forwarded while sharing is off" bash -c "! $P ping -c1 -W2 10.0.2.2"
check "no DNS query is logged" bash -c "! journalctl -b -u deltos-dnsmasq --no-pager | grep -q 'query\['"

# --- self-healing (Track E §23.5) -----------------------------------------
old=$(pid deltos-dnsmasq); kill -9 "$old"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-dnsmasq); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ] && systemctl is-active --quiet deltos-dnsmasq"; then ok "dnsmasq is restarted after a crash"; else bad "dnsmasq is restarted after a crash"; fi
check "names still resolve after the restart" bash -c "sleep 1; [ \"\$($P dig +short +time=2 +tries=3 @10.42.0.1 webos-test.deltos.lan)\" = 10.42.0.1 ]"
old=$(pid deltos-hostapd); kill -9 "$old"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-hostapd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ]"; then ok "hostapd is restarted after a crash"; else bad "hostapd is restarted after a crash"; fi
if until_t 60 joined; then ok "the phone rejoins after the radio restarts"; else bad "the phone rejoins after the radio restarts"; fi
old=$(pid deltos-deltd); kill -9 "$old"
if until_t 30 bash -c "[ \"\$($C -H 'Cookie: delt_session=vm-test-token' http://webos-test.deltos.lan/api/v1/status)\" = 200 ]"; then ok "deltd is restarted after a crash"; else bad "deltd is restarted after a crash"; fi

# --- a checked change takes effect without a reboot --------------------------
# configure_ap through authorize()/apply(), with a test stand-in for shelld.
old=$(pid deltos-hostapd); dns_before=$(pid deltos-dnsmasq)
$S/bin/configure_ap_standin /var/lib/deltos/network/registry.sqlite /run/deltos/network "DeltOS VM Test" "test-only passphrase" GB 11 >> "$OUT" 2>&1
check "configure_ap rewrote hostapd.conf (channel 11), still 0600" bash -c "grep -q '^channel=11$' /run/deltos/network/hostapd.conf && [ \"\$(stat -c %a /run/deltos/network/hostapd.conf)\" = 600 ]"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-hostapd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ]"; then ok "the watcher restarted hostapd"; else bad "the watcher restarted hostapd"; fi
if until_t 30 bash -c "iw dev wlan0 info | grep -q 'channel 11 '"; then ok "the radio is on channel 11"; else bad "the radio is on channel 11 ($(iw dev wlan0 info | grep channel))"; fi
if until_t 90 bash -c "$P iw dev wlan1 link | grep -q 'freq: 2462'"; then ok "the phone follows the hotspot to channel 11"; else bad "the phone follows the hotspot to channel 11"; fi
check "dnsmasq was not restarted by a radio change" test "$(pid deltos-dnsmasq)" = "$dns_before"

# --- outcome -----------------------------------------------------------------
info "memory in use: $(free -m | awk '/Mem:/{print $3}') MB of $(free -m | awk '/Mem:/{print $2}') MB"
for u in deltos-hostapd deltos-dnsmasq deltos-deltd; do
  info "$u memory $(systemctl show -p MemoryCurrent --value $u.service | awk '{printf "%.1f MB", $1/1048576}')"
done
journalctl -b -u 'deltos-*' --no-pager > "$S/results/journal.txt" 2>&1
if [ "$FAILS" = 0 ]; then echo "RESULT PASS" | tee -a "$OUT" > /dev/console; else echo "RESULT FAIL ($FAILS)" | tee -a "$OUT" > /dev/console; fi
sync
