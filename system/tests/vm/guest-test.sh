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
check "deltos-tls ran at boot and succeeded" bash -c "[ \"\$(systemctl show -p Result --value deltos-tls.service)\" = success ] && [ \"\$(systemctl show -p ExecMainStatus --value deltos-tls.service)\" = 0 ] && [ \"\$(systemctl show -p ExecMainExitTimestampMonotonic --value deltos-tls.service)\" != 0 ]"
check "the certificates were made before deltd started" bash -c "[ \$(systemctl show -p ExecMainExitTimestampMonotonic --value deltos-tls) -lt \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-deltd) ]"
check "the daily certificate check is scheduled" systemctl is-active --quiet deltos-tls.timer
check "no DeltOS unit failed" test "$(systemctl list-units --failed --plain --no-legend 'deltos-*' | wc -l)" = 0
for u in deltos-net-render deltos-hostapd deltos-dnsmasq deltos-identityd deltos-deltd; do
  info "$u became active $(systemctl show -p ActiveEnterTimestampMonotonic --value $u.service | awk '{printf "%.1f", $1/1e6}') s after boot"
done

# --- memory, idle, before any test touches the box --------------------------
M=$S/results/measure.txt
rss() { awk -v k="$2" '$1==k":"{print $2}' "/proc/$(pid "$1")/status"; }
{
  echo "idle_mem_used_kib=$(awk '/MemTotal/{t=$2} /MemAvailable/{a=$2} END{print t-a}' /proc/meminfo)"
  for u in deltos-hostapd deltos-dnsmasq deltos-identityd deltos-shelld deltos-deltd; do echo "idle_${u}_rss_kib=$(rss $u VmRSS)"; done
} > "$M"

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
check "the TLS keys are readable by deltd only" bash -c "[ \"\$(stat -c '%a %U %G' /var/lib/deltos/tls/deltos.lan.key)\" = '640 root deltos-deltd' ] && [ \"\$(stat -c '%a %U %G' /var/lib/deltos/tls/deltos-packs.lan.key)\" = '640 root deltos-deltd' ]"
check "the CA key is root's alone (directory 0700, key 0600)" bash -c "[ \"\$(stat -c '%a %U' /var/lib/deltos/ca)\" = '700 root' ] && [ \"\$(stat -c '%a %U' /var/lib/deltos/ca/ca.key)\" = '600 root' ]"
check "deltd cannot see the CA key" bash -c "! nsenter -t $(pid deltos-deltd) -m test -e /var/lib/deltos/ca/ca.key && ! test -e /var/lib/deltos/tls/ca.key"
FP_BOX=$(journalctl -b -u deltos-tls --no-pager -o cat | sed -n 's/^ *\([0-9A-F:]\{95\}\) *$/\1/p' | tail -1)
info "box CA fingerprint printed at boot: $FP_BOX"
# Name constraint: even with the CA key, a certificate for another site fails
T=$(mktemp -d)
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=bank.example.com -keyout $T/k -out $T/csr 2>/dev/null
printf 'subjectAltName=DNS:bank.example.com\n' > $T/ext
openssl x509 -req -in $T/csr -CA /var/lib/deltos/ca/ca.crt -CAkey /var/lib/deltos/ca/ca.key -CAserial $T/srl -CAcreateserial -days 5 -extfile $T/ext -out $T/evil.crt 2>/dev/null
check "the box CA cannot vouch for another website (name constraint)" bash -c "[ -s $T/evil.crt ] && ! openssl verify -CAfile /var/lib/deltos/ca/ca.crt $T/evil.crt && openssl verify -CAfile /var/lib/deltos/ca/ca.crt /var/lib/deltos/tls/deltos.lan.crt"
rm -rf $T
# deltos-shelld, the key holder (docs/shelld.md)
check "shelld runs as root" test "$(ps -o user= -p "$(pid deltos-shelld)")" = root
check "shelld has no network, only loopback (rule 1)" bash -c "[ \"\$(nsenter -t $(pid deltos-shelld) -n ip -o link | wc -l)\" = 1 ]"
check "shelld's socket is 0660 root:deltos-shellc" test "$(stat -c '%a %U %G' /run/deltos/shelld/shelld.sock)" = "660 root deltos-shellc"
check "shelld opens no TCP or UDP port" bash -c "! ss -Htulpn | grep -q deltos-shelld"
check "shelld started after identityd and before deltd" bash -c "[ \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-identityd) -lt \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-shelld) ] && [ \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-shelld) -lt \$(systemctl show -p ActiveEnterTimestampMonotonic --value deltos-deltd) ]"
SC=/mnt/share/system/tests/vm/shelld-call.py
check "deltd's user reaches shelld" bash -c "runuser -u deltos-deltd -- python3 $SC '{\"call\":\"list\",\"listing\":\"actions\"}' | grep -q install_addon"
check "another user cannot open shelld's socket" bash -c "! runuser -u nobody -- python3 $SC '{\"call\":\"list\",\"listing\":\"actions\"}'"
check "a user in the socket's group but not deltd is disconnected unanswered" bash -c "runuser -u deltos-identity -g deltos-shellc -- python3 $SC '{\"call\":\"list\",\"listing\":\"actions\"}'; [ \$? = 3 ]"
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
# A teacher's device trusts the box once: it fetches the CA over plain
# HTTP and compares its fingerprint with the one the box shows itself.
CA=/tmp/box-ca.pem
check "the phone downloads the box CA over plain HTTP, no session" bash -c "[ \"\$($P curl -s --max-time 10 -o /tmp/box-ca.der -w '%{http_code} %{content_type}' http://home.deltos.lan/api/v1/certificate/ca.crt)\" = '200 application/x-x509-ca-cert' ]"
openssl x509 -inform DER -in /tmp/box-ca.der -out $CA 2>/dev/null
FP_PHONE=$(openssl x509 -inform DER -in /tmp/box-ca.der -noout -fingerprint -sha256 2>/dev/null | sed 's/.*=//')
check "its fingerprint is the one the box printed" bash -c "[ -n '$FP_BOX' ] && [ '$FP_PHONE' = '$FP_BOX' ]"
check "the box API states the same fingerprint" bash -c "$P curl -s --max-time 10 http://home.deltos.lan/api/v1/certificate | grep -q '\"fingerprint_sha256\":\"$FP_BOX\"'"
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
# deltd's side of the boundary, by hand: the admin's live HTTPS session
# taken to shelld the way deltd will take it. shelld asks identityd itself.
SA() { runuser -u deltos-deltd -- python3 $SC "$1" 2>/dev/null; }
ACT() { SA "{\"call\":\"act\",\"action\":\"$1\",\"args\":$2,\"session\":\"$3\",\"over_https\":$4}"; }
JOB() { local id i=0 j; id=$(echo "$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["ok"]["job"])') || return 1
  while [ $i -lt 120 ]; do j=$(SA "{\"call\":\"job\",\"job\":\"$id\",\"session\":\"$ADMIN_COOKIE\",\"over_https\":true}")
    echo "$j" | grep -q finished_at && { echo "$j"; return 0; }; i=$((i+1)); sleep 1; done; return 1; }
NAME='{"name":"printer.deltos.lan","target":"10.42.0.20","requested_by":"vm"}'
ZERO=$(printf '0%.0s' $(seq 64))
r=$(ACT approve_name "$NAME" "$ZERO" true); echo "# $r" >> "$OUT"
check "shelld refuses a made-up session (not_signed_in)" bash -c "echo '$r' | grep -q not_signed_in"
r=$(ACT approve_name "$NAME" "$ADMIN_COOKIE" false)
check "shelld refuses the admin's session said to be plain HTTP" bash -c "echo '$r' | grep -q secure_connection_required"
r=$(ACT approve_name "$NAME" "$ADMIN_COOKIE" true); echo "# $r" >> "$OUT"
check "shelld approves a name with the admin's HTTPS session" bash -c "echo '$r' | grep -q '\"ok\"'"
if until_t 20 bash -c "[ \"\$($P dig +short +time=2 +tries=1 @10.42.0.1 printer.deltos.lan | head -1)\" = 10.42.0.20 ]"; then ok "the phone resolves the name shelld approved, with no restart by hand"; else bad "the phone resolves the name shelld approved"; fi
# a drive, as mount_removable will make one (not built yet: docs/shelld.md §6)
mkdir -p /run/deltos/drives/usb1 && mount -t tmpfs -o size=256m tmpfs /run/deltos/drives/usb1
r=$(SA "{\"call\":\"list\",\"listing\":\"drives\",\"session\":\"$ADMIN_COOKIE\",\"over_https\":true}")
check "shelld lists the drive by id" bash -c "echo '$r' | grep -q '\"id\":\"usb1\"'"
t0=$(date +%s)
j=$(JOB "$(ACT start_backup '{"drive":"usb1","passphrase":"vm shelld backup passphrase"}' "$ADMIN_COOKIE" true)"); echo "# $j" >> "$OUT"
info "backup through shelld took $(( $(date +%s) - t0 )) s"
check "the admin backs up to the drive through shelld (a job)" bash -c "echo '$j' | grep -q '\"state\":\"done\"' && [ \"\$(ls /run/deltos/drives/usb1 | wc -l)\" = 1 ]"
: > /run/deltos/drives/usb1/notes-0.1.0.deltos-addon; echo x > /run/deltos/drives/usb1/notes-0.1.0.deltos-addon.minisig
r=$(SA "{\"call\":\"list\",\"listing\":\"addon_sources\",\"session\":\"$ADMIN_COOKIE\",\"over_https\":true}")
check "shelld lists the add-on bundle on the drive" bash -c "echo '$r' | grep -q 'drive-usb1/notes-0.1.0.deltos-addon'"
j=$(JOB "$(ACT install_addon '{"source":"drive-usb1/notes-0.1.0.deltos-addon","id":"notes","version":"0.1.0"}' "$ADMIN_COOKIE" true)"); echo "# $j" >> "$OUT"
check "an add-on is refused while the box has no add-on key (failed_signature)" bash -c "echo '$j' | grep -q failed_signature && [ ! -e /usr/lib/deltos/apps/notes ]"
umount /run/deltos/drives/usb1
L=/var/lib/deltos/shelld/actions.log
check "the action log names the admin, with every result, and holds no secret" bash -c "grep -q '\"action\":\"approve_name\".*\"result\":\"ok\"' $L && grep -q '\"result\":\"not_signed_in\"' $L && grep -q '\"action\":\"start_backup\".*\"result\":\"ok\"' $L && ! grep -q -e '$ADMIN_PW' -e 'vm shelld backup passphrase' $L && [ \"\$(stat -c '%a %U' $L)\" = '640 root' ]"
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

# --- certificate renewal ------------------------------------------------------
# Nothing due: nothing changes and deltd is left alone.
old=$(pid deltos-deltd); before=$(sha256sum /var/lib/deltos/tls/deltos.lan.crt)
systemctl start deltos-tls.service
sleep 3
check "a daily check with nothing due changes nothing and leaves deltd running" bash -c "[ \"\$(sha256sum /var/lib/deltos/tls/deltos.lan.crt)\" = '$before' ] && [ \"\$(systemctl show -p MainPID --value deltos-deltd)\" = $old ]"
# A server certificate gone (as if renewal were due): reissued from the same
# CA, deltd restarted onto it, and a device that trusted the box once still does.
rm /var/lib/deltos/tls/deltos.lan.crt
systemctl start deltos-tls.service
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-deltd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ] && systemctl is-active --quiet deltos-deltd"; then ok "a reissued certificate restarts deltd"; else bad "a reissued certificate restarts deltd"; fi
if until_t 30 bash -c "$P curl -s --max-time 5 --cacert $CA -o /dev/null https://home.deltos.lan/api/v1/openapi.json"; then ok "the renewed certificate is trusted by the device that trusted the box once"; else bad "the renewed certificate is trusted by the device that trusted the box once"; fi
check "the CA did not change" bash -c "[ \"\$(openssl x509 -in /var/lib/deltos/ca/ca.crt -noout -fingerprint -sha256 | sed 's/.*=//')\" = '$FP_BOX' ]"

# --- a checked change takes effect without a reboot --------------------------
# configure_ap through authorize()/apply(), with a test stand-in for shelld.
old=$(pid deltos-hostapd); dns_before=$(pid deltos-dnsmasq)
$S/bin/configure_ap_standin /var/lib/deltos/network/registry.sqlite /run/deltos/network "DeltOS VM Test" "test-only passphrase" GB 11 >> "$OUT" 2>&1
check "configure_ap rewrote hostapd.conf (channel 11), still 0600" bash -c "grep -q '^channel=11$' /run/deltos/network/hostapd.conf && [ \"\$(stat -c %a /run/deltos/network/hostapd.conf)\" = 600 ]"
if until_t 30 bash -c "p=\$(systemctl show -p MainPID --value deltos-hostapd); [ \"\$p\" != 0 ] && [ \"\$p\" != $old ]"; then ok "the watcher restarted hostapd"; else bad "the watcher restarted hostapd"; fi
if until_t 30 bash -c "iw dev wlan0 info | grep -q 'channel 11 '"; then ok "the radio is on channel 11"; else bad "the radio is on channel 11 ($(iw dev wlan0 info | grep channel))"; fi
if until_t 90 bash -c "$P iw dev wlan1 link | grep -q 'freq: 2462'"; then ok "the phone follows the hotspot to channel 11"; else bad "the phone follows the hotspot to channel 11"; fi
check "dnsmasq was not restarted by a radio change" test "$(pid deltos-dnsmasq)" = "$dns_before"

# --- backup and restore (docs/box-backup.md) ----------------------------------
# A folder stands in for the mounted USB drive. The backup is made after the
# channel change above, so the restore must bring channel 11 back too.
DRIVE=/srv/backup-drive; mkdir -p $DRIVE
BK=/usr/lib/deltos/bin/deltos-backup
BPASS="vm test backup passphrase"
t0=$(date +%s)
check "root makes an encrypted backup at the console" bash -c "echo '$BPASS' | $BK create --to $DRIVE --passphrase-stdin"
info "backup took $(( $(date +%s) - t0 )) s"
BID=$($BK list $DRIVE | awk 'NR==1{print $1}')
info "backup $BID: $(stat -c %s $DRIVE/$BID 2>/dev/null) bytes; the pack is $(stat -c %s /var/lib/deltos/packs/science.wax) bytes"
check "the drive holds the one backup file and nothing else" test "$(ls -A $DRIVE | wc -l)" = 1
check "nothing in it is readable without the passphrase" bash -c "head -c 22 $DRIVE/$BID | grep -q '^age-encryption.org/v1' && ! grep -a -q -e 'SQLite format' -e 'DeltOS VM Test' -e 'test-only passphrase' -e 'BEGIN' $DRIVE/$BID"
check "the backup file is the admin's alone (0600)" test "$(stat -c %a $DRIVE/$BID)" = 600
check "a wrong passphrase is refused" bash -c "! echo 'not the right passphrase' | $BK review --from $DRIVE $BID --passphrase-stdin"
check "a non-root user cannot make a backup" bash -c "! echo '$BPASS' | runuser -u nobody -- $BK create --to /tmp --passphrase-stdin"
SSL_FP() { openssl x509 -in /var/lib/deltos/ca/ca.crt -noout -fingerprint -sha256 | sed 's/.*=//'; }
FP_OLD=$(SSL_FP)

# A fresh box: every piece of state thrown away and set up again from
# nothing, with a different Wi-Fi name, a new first admin and (at start) a
# new certificate authority. The pack file stays, as on a box whose SD card
# kept its packs; the review must notice it is there.
systemctl stop deltos-deltd deltos-shelld deltos-identityd deltos-hostapd deltos-dnsmasq deltos-uplink-firewall deltos-hotspot-address deltos-net-render deltos-net-changed.path
rm -rf /var/lib/deltos/identity /var/lib/deltos/ca /var/lib/deltos/tls /var/lib/deltos/network /var/lib/deltos/b4 /var/lib/deltos/shelld /var/lib/deltos/apps
systemd-tmpfiles --create /usr/lib/tmpfiles.d/deltos.conf
cat > /root/fresh.conf <<'CONF'
mode=hotspot
interface=wlan0
address=10.42.0.1/24
uplink_interface=eth0
ssid=Fresh Box
passphrase=fresh box passphrase
country=GB
channel=1
CONF
/usr/lib/deltos/bin/deltos-net-provision /var/lib/deltos/network/registry.sqlite /root/fresh.conf >/dev/null
rm /root/fresh.conf
runuser -u deltos-identity -- /usr/lib/deltos/bin/deltos-identity-provision --display-name "Fresh Admin" --sign-in-name fresh > /root/fresh-admin.txt
FRESH_PW=$(sed -n 's/^Password, shown this once and never stored: //p' /root/fresh-admin.txt)
systemctl start deltos-box.target
until_t 120 systemctl is-active --quiet deltos-deltd.service
FP_FRESH=$(SSL_FP)
info "fresh box CA: $FP_FRESH"
check "the fresh box made its own, different certificate stamp" bash -c "[ -n '$FP_FRESH' ] && [ '$FP_FRESH' != '$FP_OLD' ]"
HS="curl -s -o /dev/null -w %{http_code} --max-time 10 --resolve home.deltos.lan:443:127.0.0.1"
check "a device that trusted the old box does not trust the fresh one" bash -c "! $HS --cacert $CA https://home.deltos.lan/api/v1/openapi.json"

# Review, then restore.
echo "$BPASS" | $BK review --from $DRIVE $BID --passphrase-stdin > /tmp/review.txt 2>&1
cat /tmp/review.txt >> "$OUT"
RID=$(sed -n 's/^To restore it: deltos-backup restore //p' /tmp/review.txt)
check "the review shows the backup's stamp, people, Wi-Fi and pack" bash -c "grep -q 'certificate (stamp): $FP_OLD' /tmp/review.txt && grep -q 'admins who can sign in: admin' /tmp/review.txt && grep -q 'Wi-Fi name \"DeltOS VM Test\"' /tmp/review.txt && grep -q '^  pack ' /tmp/review.txt && ! grep -q reinstall /tmp/review.txt"
check "the review changed nothing live" test "$(SSL_FP)" = "$FP_FRESH"
check "a restore is not started without typing restore" bash -c "! echo no | $BK restore $RID"
t0=$(date +%s)
check "root restores the reviewed backup" bash -c "$BK restore $RID --yes"
info "restore took $(( $(date +%s) - t0 )) s"
for u in deltos-identityd deltos-deltd deltos-hostapd deltos-dnsmasq; do
  if until_t 90 systemctl is-active --quiet "$u.service"; then ok "$u is running after the restore"; else bad "$u is running after the restore ($(systemctl is-active $u.service))"; fi
done
check "the certificate stamp is the backed-up box's again" test "$(SSL_FP)" = "$FP_OLD"
check "no DeltOS unit failed after the restore" test "$(systemctl list-units --failed --plain --no-legend 'deltos-*' | wc -l)" = 0
check "owners and modes came back (CA root 0700/0600, register deltos-identity 0700, server keys root:deltos-deltd 0640)" bash -c "[ \"\$(stat -c '%a %U' /var/lib/deltos/ca)\" = '700 root' ] && [ \"\$(stat -c '%a %U' /var/lib/deltos/ca/ca.key)\" = '600 root' ] && [ \"\$(stat -c '%a %U' /var/lib/deltos/identity)\" = '700 deltos-identity' ] && [ \"\$(stat -c '%a %U' /var/lib/deltos/identity/identity.sqlite)\" = '600 deltos-identity' ] && [ \"\$(stat -c '%a %U %G' /var/lib/deltos/tls/deltos.lan.key)\" = '640 root deltos-deltd' ] && [ \"\$(stat -c '%a %U' /var/lib/deltos/network)\" = '700 root' ]"
if until_t 60 bash -c "$HS --cacert $CA https://home.deltos.lan/api/v1/openapi.json | grep -q 200"; then ok "a device that trusted the box before the restore trusts it again, with nothing to set up"; else bad "a device that trusted the box before the restore trusts it again"; fi
if until_t 60 bash -c "iw dev wlan0 info | grep -q 'ssid DeltOS VM Test' && iw dev wlan0 info | grep -q 'channel 11 '"; then ok "the hotspot's name and channel came back"; else bad "the hotspot's name and channel came back"; fi
if until_t 120 joined && until_t 60 bash -c "$P iw dev wlan1 link | grep -q 'freq: 2462'"; then ok "the phone rejoins the restored hotspot on its own"; else bad "the phone rejoins the restored hotspot on its own"; fi
$P busybox udhcpc -i wlan1 -n -q -f -t 10 -T 2 -s /tmp/udhcpc.sh >/dev/null 2>&1 || true
rm -f $J/admin $J/admin.csrf
if until_t 30 bash -c "[ \"\$(W $J/admin https POST /session '{\"sign_in_name\":\"admin\",\"password\":\"$ADMIN_PW\"}')\" = 200 ]"; then ok "the backed-up admin signs in over HTTPS from the phone with their old password"; else bad "the backed-up admin signs in over HTTPS with their old password ($(cat $J/admin.out 2>/dev/null))"; fi
check "the fresh box's throwaway admin is gone" bash -c "[ \"\$(W $J/fresh https POST /session '{\"sign_in_name\":\"fresh\",\"password\":\"$FRESH_PW\"}')\" != 200 ]"
rm -f $J/phone.csrf
check "the student's remembered phone signs in again without the card" test "$(W $J/phone http POST /session '')" = 200
check "search still finds the page after the restore" bash -c "[ \"\$(G $J/phone http '/search?q=evaporation')\" = 200 ] && grep -q 'Water cycle' $J/phone.out"
check "the replaced state is kept once, root-only" test "$(stat -c '%a %U' /var/lib/deltos/.restore-previous)" = "700 root"
rm -f /root/fresh-admin.txt

# --- memory after every test, and a class signing in at once ---------------
{
  echo "after_mem_used_kib=$(awk '/MemTotal/{t=$2} /MemAvailable/{a=$2} END{print t-a}' /proc/meminfo)"
  for u in deltos-hostapd deltos-dnsmasq deltos-identityd deltos-shelld deltos-deltd; do echo "after_${u}_rss_kib=$(rss $u VmRSS) peak_kib=$(rss $u VmHWM)"; done
  # identityd's own code, in a separate process: 40 sign-ins at once on this
  # machine's cores. Memory only; times under emulation mean nothing.
  [ -x $S/bin/signin_burst ] && $S/bin/signin_burst 40 shared
} >> "$M" 2>&1

# --- outcome -----------------------------------------------------------------
info "memory in use: $(free -m | awk '/Mem:/{print $3}') MB of $(free -m | awk '/Mem:/{print $2}') MB"
for u in deltos-hostapd deltos-dnsmasq deltos-identityd deltos-deltd; do
  info "$u memory $(systemctl show -p MemoryCurrent --value $u.service | awk '{printf "%.1f MB", $1/1048576}')"
done
journalctl -b -u 'deltos-*' --no-pager > "$S/results/journal.txt" 2>&1
if [ "$FAILS" = 0 ]; then echo "RESULT PASS" | tee -a "$OUT" > /dev/console; else echo "RESULT FAIL ($FAILS)" | tee -a "$OUT" > /dev/console; fi
sync
