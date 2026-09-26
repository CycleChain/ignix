#!/usr/bin/env bash
# Sunucu isteyen testler (tests/large_payloads.rs ve #[ignore] ile işaretli ağ testleri) için:
# ignix'i geçici bir dizinde başlatır, `cargo test -- --include-ignored` koşar, sunucuyu durdurur.
#   bash .hub/sunucu-testleri.sh                        # tüm testler
#   bash .hub/sunucu-testleri.sh --test large_payloads  # argümanlar cargo test'e gider
# Port sabittir (127.0.0.1:7379). SO_REUSEPORT yüzünden ikinci bir sunucu hata vermeden aynı
# portu paylaşabileceği için port doluysa hiç başlamaz.
set -u

if lsof -nP -iTCP:7379 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "7379 portu dolu: önce oradaki sunucuyu durdur (lsof -nP -iTCP:7379 -sTCP:LISTEN)." >&2
  exit 1
fi

cargo build --bin ignix || exit 1
bin="${CARGO_TARGET_DIR:-$PWD/target}/debug/ignix"
dir="$(mktemp -d)"

# Sunucu çalışma dizinine ignix.aof yazar; geçici dizin koşudan sonra silinir.
(cd "$dir" && exec "$bin") >"$dir/sunucu.log" 2>&1 &
srv=$!
trap 'kill "$srv" 2>/dev/null; wait "$srv" 2>/dev/null; rm -rf "$dir"' EXIT

for _ in {1..50}; do
  nc -z 127.0.0.1 7379 2>/dev/null && break
  sleep 0.2
done
if ! kill -0 "$srv" 2>/dev/null; then
  echo "ignix başlamadı:" >&2
  cat "$dir/sunucu.log" >&2
  exit 1
fi

cargo test "$@" -- --include-ignored
