#!/usr/bin/env python3
# Send one request line to deltos-shelld and print the answer line, the way
# deltd will (docs/shelld.md §3). Exit 3 if the socket closes unanswered.
#   shelld-call.py '{"call":"list","listing":"actions"}'
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(30)
s.connect(sys.argv[2] if len(sys.argv) > 2 else "/run/deltos/shelld/shelld.sock")
buf = b""
try:
    s.sendall(sys.argv[1].encode() + b"\n")
    while not buf.endswith(b"\n"):
        c = s.recv(65536)
        if not c:
            sys.exit(3)
        buf += c
except (ConnectionResetError, BrokenPipeError):
    # shelld closed the connection without an answer
    sys.exit(3)
sys.stdout.write(buf.decode())
