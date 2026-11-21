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
for u in deltos-net-render deltos-hotspot-address deltos-uplink-firewall deltos-hostapd deltos-dnsmasq deltos-identityd deltos-deltd; do
  if until_t 120 systemctl is-active --quiet "$u.service"; then ok "$u started at boot"; else bad "$u started at boot ($(systemctl is-active $u.service))"; journalctl -b -u $u --no-pager | tail -20 >> "$OUT"; fi
done
check "deltos-net-changed.path watching" systemctl is-active --quiet deltos-net-changed.path
check "no DeltOS unit failed" test "$(systemctl list-units --failed --plain --no-legend 'deltos-*' | wc -l)" = 0
for u in deltos-net-render deltos-hostapd deltos-dnsmasq deltos-identityd deltos-deltd; do
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
check "identityd runs as deltos-identity" test "$(ps -o user= -p "$(pid deltos-identityd)")" = deltos-identity
check "identityd holds no powers at all" grep -q '^CapEff:[[:space:]]*0000000000000000$' "/proc/$(pid deltos-identityd)/status"
check "identityd has no network, only loopback" bash -c "[ \"\$(nsenter -t $(pid deltos-identityd) -n ip -o link | wc -l)\" = 1 ]"
check "identityd started before deltd" bash -c "[ \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-identityd) -lt \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-deltd) ]"
check "the register is its owner's alone (0700, files 0600)" bash -c "[ \"\$(stat -c '%a %U' /var/lib/deltos/identity)\" = '700 deltos-identity' ] && [ \"\$(stat -c %a /var/lib/deltos/identity/identity.sqlite)\" = 600 ] && [ \"\$(stat -c %a /var/lib/deltos/identity/card-key)\" = 600 ]"
check "identityd's socket is 0660 deltos-identity:deltos-identc" test "$(stat -c '%a %U %G' /run/deltos/identity/identityd.sock)" = "660 deltos-identity deltos-identc"
check "deltos-deltd is in deltos-identc" bash -c 'id -nG deltos-deltd | grep -qw deltos-identc'
check "deltd cannot see the register" bash -c "! nsenter -t $(pid deltos-deltd) -m test -e /var/lib/deltos/identity/identity.sqlite"
check "another user cannot reach identityd's socket" bash -c "! runuser -u nobody -- python3 -c \"import socket;s=socket.socket(socket.AF_UNIX);s.connect('/run/deltos/identity/identityd.sock')\""
check "the TLS keys are readable by deltd only" test "$(stat -c '%a %U %G' /var/lib/deltos/tls/deltos.lan.key)" = "640 root deltos-deltd"
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
# joined = the key handshake finished (associating alone happens even with a wrong password)
joined() { $P wpa_cli -p /run/wpa-phone -i wlan1 status 2>/dev/null | grep -q '^wpa_state=COMPLETED$'; }

$P wpa_supplicant -B -i wlan1 -c /tmp/phone-wrong.conf -P /run/wpa-phone.pid >/dev/null
if until_t 25 joined; then bad "a wrong Wi-Fi password is refused"; else ok "a wrong Wi-Fi password is refused"; fi
check "the box saw the wrong password" bash -c "journalctl -b -u deltos-hostapd --no-pager | grep -q AP-STA-POSSIBLE-PSK-MISMATCH"
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
check "WebOS name resolves to the box" test "$(dig1 home.deltos.lan)" = 10.42.0.1
check "a fresh pack hostname resolves to the box" test "$(dig1 pack-3-g1-0123456789abcdef.deltos-packs.lan)" = 10.42.0.1
check "an outside name is 'no such name' at once (offline)" bash -c "$P dig +time=2 +tries=1 @10.42.0.1 example.com | grep -q 'status: NXDOMAIN'"
check "Android's internet check resolves to the box" test "$(dig1 connectivitycheck.gstatic.com)" = 10.42.0.1

C="$P curl -s -o /dev/null -w %{http_code} --max-time 5"
CA=/etc/deltos-vm-test/ca.crt
ADMIN_PW=$(cat /root/admin-pw)
J=/tmp/jars; mkdir -p $J
# a JSON field from a file (python3 is in the image)
jf() { python3 -c "import json,sys; v=json.load(open('$1')); print(v$2)" 2>/dev/null; }
# a write: the WebOS's own origin, and the session's anti-forgery token once signed in
W() { local jar=$1 scheme=$2 method=$3 path=$4 data=${5:-}
      local csrf="" body=()
      [ -f "$jar.csrf" ] && csrf=$(cat "$jar.csrf")
      [ -n "$data" ] && body=(-H 'Content-Type: application/json' -d "$data")
      $P curl -s --max-time 10 --cacert $CA -b "$jar" -c "$jar" -o "$jar.out" -w '%{http_code}' -X "$method" \
        -H "Origin: $scheme://home.deltos.lan" -H "X-Delt-CSRF: $csrf" "${body[@]}" \
        "$scheme://home.deltos.lan/api/v1$path"
      t=$(jf "$jar.out" "['csrf_token']"); [ -n "$t" ] && [ "$t" != None ] && echo "$t" > "$jar.csrf"; }
G() { $P curl -s --max-time 10 --cacert $CA -b "$1" -c "$1" -o "$1.out" -w '%{http_code}' "$2://home.deltos.lan/api/v1$3"; }
# the checks run their commands in a child bash, which needs these too
export P CA J
export -f jf W G

check "box API refuses a phone with no session (401)" test "$($C http://home.deltos.lan/api/v1/status)" = 401
check "an admin password over plain HTTP is refused (403 secure_connection_required)" bash -c "[ \"\$(W $J/plain http POST /session '{\"sign_in_name\":\"admin\",\"password\":\"$ADMIN_PW\"}')\" = 403 ] && grep -q secure_connection_required $J/plain.out"
check "HTTPS answers with the box's deltos.lan certificate" bash -c "$P curl -s --max-time 10 --cacert $CA -o /dev/null https://home.deltos.lan/api/v1/openapi.json"
code=$(W $J/admin https POST /session "{\"sign_in_name\":\"admin\",\"password\":\"$ADMIN_PW\"}")
check "the admin signs in over HTTPS to a secure session" bash -c "[ $code = 200 ] && [ \"\$(jf $J/admin.out \"['secure']\")\" = True ]"
check "the secure cookie is __Host- and Secure" grep -q '__Host-delt_session' $J/admin
check "HTTPS answers carry HSTS for the WebOS name only" bash -c "$P curl -s -D - -o /dev/null --cacert $CA -b $J/admin https://home.deltos.lan/api/v1/session | grep -qi '^strict-transport-security: max-age=31536000'"
code=$(W $J/admin https POST /profiles '{"display_name":"Amara","kind":"passwordless","class_or_group":"5a","roles":[{"role":"student"}]}')
STUDENT=$(jf $J/admin.out "['profile_id']")
check "the admin adds a student (201)" test "$code" = 201
code=$(W $J/admin https POST "/classes/5a/members/$STUDENT/sign-in-card" '')
CARD=$(jf $J/admin.out "['code']")
check "the admin issues the student a sign-in card" bash -c "[ $code = 201 ] && [[ '$CARD' =~ ^[0-9A-Z]{5}-[0-9A-Z]{5}$ ]]"
ADMIN_COOKIE=$(awk '$6=="__Host-delt_session"{print $7}' $J/admin)
check "a card cannot be issued over plain HTTP, even with the admin's cookie replayed (403)" bash -c "[ \"\$($P curl -s -o /dev/null -w %{http_code} --max-time 10 -X POST -H 'Origin: http://home.deltos.lan' -H \"X-Delt-CSRF: \$(cat $J/admin.csrf)\" -H 'Cookie: delt_session=$ADMIN_COOKIE' http://home.deltos.lan/api/v1/classes/5a/members/$STUDENT/sign-in-card)\" = 403 ]"
code=$(W $J/phone http POST /session "{\"sign_in_code\":\"$CARD\"}")
check "the student signs in on the phone with the card, over plain HTTP" bash -c "[ $code = 200 ] && [ \"\$(jf $J/phone.out \"['secure']\")\" = False ]"
check "the phone is remembered (delt_device)" grep -q 'delt_device' $J/phone
check "box API answers the signed-in phone (200)" test "$(G $J/phone http /status)" = 200
check "search across packs finds the page" bash -c "[ \"\$(G $J/phone http '/search?q=evaporation')\" = 200 ] && grep -q 'Water cycle' $J/phone.out"
G $J/phone http /packs >/dev/null; PUUID=$(jf $J/phone.out "['items'][0]['archive_uuid']")
G $J/phone http "/packs/$PUUID" >/dev/null; PHOST=$(jf $J/phone.out "['pack_hostname']")
info "phone's pack hostname: $PHOST"
check "the phone reads a page on its own pack hostname" test "$($C -L http://$PHOST/)" = 200
check "the tap-your-name list is refused to the phone" test "$(G $J/phone http /sign-in/profiles)" = 403
check "the tap-your-name list answers the box's own screen" bash -c "[ \"\$(curl -s -o /dev/null -w %{http_code} --max-time 10 --cacert $CA --resolve home.deltos.lan:443:127.0.0.1 https://home.deltos.lan/api/v1/sign-in/profiles)\" = 200 ]"
check "a write from another site is rejected" bash -c "[ \"\$($P curl -s -o /dev/null -w %{http_code} --max-time 10 -b $J/phone -X DELETE -H 'Origin: http://evil.example' http://home.deltos.lan/api/v1/session)\" = 403 ]"
check "the phone signs out" test "$(W $J/phone http DELETE /session '')" = 200
sleep 1
check "after sign-out the phone's pack hostname is shut" bash -c "[ \"\$($C -L http://$PHOST/)\" != 200 ]"
check "after sign-out the API refuses the phone (401)" test "$(G $J/phone http /status)" = 401
rm -f $J/phone.csrf
check "the remembered phone starts a new session without the card" test "$(W $J/phone http POST /session '')" = 200
check "Android's check is answered 'internet works' (204)" test "$($C http://connectivitycheck.gstatic.com/generate_204)" = 204
check "Apple's check is answered 'Success'" bash -c "$P curl -s --max-time 5 http://captive.apple.com/hotspot-detect.html | grep -q Success"
check "a name deltd does not serve is refused" test "$($C -H 'Host: evil.example' http://10.42.0.1/)" = 404
check "nothing from the hotspot is forwarded while sharing is off" bash -c "! $P ping -c1 -W2 10.0.2.2"
check "no DNS query is logged" bash -c "! journalctl -b -u deltos-dnsmasq --no-pager | grep -q 'query\['"

# --- self-healing (Track E §23.5) -----------------------------------------
old=$(pid deltos-dnsmasq); kill -9 "$old"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-dnsmasq); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ] && systemctl is-active --quiet deltos-dnsmasq"; then ok "dnsmasq is restarted after a crash"; else bad "dnsmasq is restarted after a crash"; fi
# under emulation dnsmasq takes a few seconds after "active" before it answers
if until_t 20 bash -c "[ \"\$($P dig +short +time=1 +tries=1 @10.42.0.1 home.deltos.lan)\" = 10.42.0.1 ]"; then ok "names still resolve after the restart"; else bad "names still resolve after the restart"; fi
old=$(pid deltos-hostapd); kill -9 "$old"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-hostapd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ]"; then ok "hostapd is restarted after a crash"; else bad "hostapd is restarted after a crash"; fi
if until_t 60 joined; then ok "the phone rejoins after the radio restarts"; else bad "the phone rejoins after the radio restarts"; fi
old=$(pid deltos-deltd); kill -9 "$old"
if until_t 30 bash -c "[ \"\$(G $J/phone http /status)\" = 200 ]"; then ok "deltd is restarted after a crash, the phone still signed in"; else bad "deltd is restarted after a crash"; fi
old=$(pid deltos-identityd); kill -9 "$old"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-identityd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ] && systemctl is-active --quiet deltos-identityd"; then ok "identityd is restarted after a crash"; else bad "identityd is restarted after a crash"; fi
sleep 6
check "sessions survive an identityd restart (they are in the register)" test "$(G $J/phone http /status)" = 200

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
for u in deltos-hostapd deltos-dnsmasq deltos-identityd deltos-deltd; do
  info "$u memory $(systemctl show -p MemoryCurrent --value $u.service | awk '{printf "%.1f MB", $1/1048576}')"
done
journalctl -b -u 'deltos-*' --no-pager > "$S/results/journal.txt" 2>&1
if [ "$FAILS" = 0 ]; then echo "RESULT PASS" | tee -a "$OUT" > /dev/console; else echo "RESULT FAIL ($FAILS)" | tee -a "$OUT" > /dev/console; fi
sync
