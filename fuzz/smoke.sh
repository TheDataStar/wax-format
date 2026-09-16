#!/usr/bin/env sh
# Short fuzz smoke pass over all three targets that leaves the tree CLEAN.
#
# Why this script exists: libFuzzer writes every newly discovered input into
# the FIRST corpus directory it is given, and treats the remaining ones as
# read-only. `cargo fuzz run <target>` defaults that first directory to the
# committed fuzz/corpus/<target>, so a routine smoke pass silently grows the
# checked-in seed corpus and shows up as ~200 pending changes.
#
# So we hand it a git-ignored scratch directory first and the committed seeds
# second. Discoveries land in scratch; the seeds stay exactly as committed.
# Growing the seed corpus is then a deliberate act, not a side effect.
#
# usage: fuzz/smoke.sh [seconds-per-target]     (default 45)
set -eu
SECS="${1:-45}"
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/fuzz/smoke-corpus"

# Windows: the targets are built with AddressSanitizer and will not LAUNCH --
# exit 0xc0000135, STATUS_DLL_NOT_FOUND -- unless the MSVC sanitizer runtime
# clang_rt.asan_dynamic-x86_64.dll is on PATH. It ships with the VC build
# tools, so locate them and prepend. A build failure this is not.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    VSWHERE="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
    if [ -x "$VSWHERE" ]; then
      VSDIR=$("$VSWHERE" -latest -products '*' \
                -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
                -property installationPath 2>/dev/null | tr -d '\r')
      if [ -n "$VSDIR" ]; then
        VSDIR=$(cygpath -u "$VSDIR")
        for d in "$VSDIR"/VC/Tools/MSVC/*/bin/Hostx64/x64; do
          [ -f "$d/clang_rt.asan_dynamic-x86_64.dll" ] && { PATH="$d:$PATH"; export PATH; break; }
        done
      fi
    fi
    command -v clang_rt.asan_dynamic-x86_64.dll >/dev/null 2>&1 || true
    ;;
esac

rc=0
for t in header-parse index-loader segment-merge; do
  mkdir -p "$OUT/$t"
  printf '\n===== %s (%ss) =====\n' "$t" "$SECS"
  # scratch corpus FIRST (written to), committed seeds SECOND (read only)
  cargo +nightly fuzz run "$t" "$OUT/$t" "$ROOT/fuzz/corpus/$t" \
      -- -max_total_time="$SECS" || rc=$?
done

printf '\n===== working tree =====\n'
DIRTY=$(cd "$ROOT" && git status --porcelain | wc -l | tr -d ' ')
if [ "$DIRTY" = "0" ]; then
  echo "clean - the committed seed corpus was not touched"
else
  echo "DIRTY ($DIRTY paths) - the smoke pass changed tracked files:"
  (cd "$ROOT" && git status --porcelain | head -20)
  rc=1
fi
exit "$rc"
