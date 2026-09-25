---
name: derleme-ve-test
description: Ignix'i derleme, test etme, lint ve biçim denetimi. Gerçek cargo komutları ve bugünkü main'deki sonuçları, test düzeni (birim, entegrasyon, sunucu isteyen ağ testleri), sunucuyu güvenle başlatma, sandbox'ta cargo, macOS'ta derlenmeyen io_uring kodu. Kod değiştirdikten sonra neyin nasıl doğrulanacağını bilmek için kullan.
---

# Derleme ve test

Kararlı Rust yeterlidir (README 1.80+ der; 1.90 ile denendi). Python ve Node.js yalnızca
örnek istemciler ve `benchmarks/` betikleri içindir.

## Komutlar ve bugünkü `main`

| Amaç | Komut | Bugünkü `main` |
|---|---|---|
| Bağımlılıklar | `cargo fetch` | geçer |
| Kütüphane, ikili, örnekler | `cargo check --lib --bins --examples` | geçer |
| Tüm hedefler (Hub kapısı `check`) | `cargo check --all-targets` | kalır: `tests/basic.rs`, `benches/exec.rs` |
| Lint (Hub kapısı `clippy`) | `cargo clippy -- -D warnings` | kalır: `src/protocol.rs:200` never_loop, `src/net.rs:65` redundant_locals, `src/net.rs:184` redundant_pattern_matching |
| Biçim (Hub kapısı `fmt`) | `cargo fmt --check` | kalır: `src/` ve `tests/` altında 11 dosya |
| Sunucusuz testler (Hub profili `test`) | `cargo test -- --skip test_large_payload` | kalır: `tests/basic.rs` derlenmez |
| Bugün çalışan test alt kümesi | `cargo test --lib --bins --test resp` | geçer (2 test) |
| Sunucu isteyen testler (Hub profili `sunucu`) | `bash .hub/sunucu-testleri.sh` | kalır: `tests/basic.rs` derlenmez |
| Benchmark (Hub profili `bench`) | `CRITERION_HOME=target/criterion cargo bench --bench '*' -- --noplot` | kalır: `benches/exec.rs` derlenmez |

`tests/basic.rs` ve `benches/exec.rs`, `Shard::exec`'in eski imzasını (tek argüman, dönüş
değeri) kullanır; güncel imza `exec(&self, cmd: Cmd, out: &mut BytesMut)`. Bu kırıklar, clippy
uyarıları ve biçim farkları ayrı bir bakım işidir. Kartın konusu değilse düzeltme; bir kapı ya da
profil yalnızca bu nedenlerle kalıyorsa `needs_human` ile "bu kartta düzelt / ayrı bakım kartı /
iptal" diye sor.

Kapılar `.hub/project.yaml` içinde sırayla koşar ve ilk kalanda durur. `cargo fmt` tüm depoyu
yeniden biçimler; bakım işi dışında yalnızca değiştirdiğin satırları rustfmt üslubunda yaz.

## Test düzeni

- Birim testleri kaynak dosyada `#[cfg(test)] mod tests` (bugün yalnızca
  `src/shard.rs::test_shard_alignment`). Bu testi silme ya da taşıma: Hub test sayısını cargo
  çıktısındaki ilk `test result:` satırından (kütüphanenin birim testleri) okur; orada 0 görünürse
  kanıt "hiç test çalışmadı" sayılır.
- Entegrasyon testleri `tests/*.rs`, her dosya ayrı bir test ikilisi: `basic.rs` (`Shard::exec`),
  `resp.rs` (ayrıştırıcı), `large_payloads.rs` (100 KB, 1 MB, 10 MB SET/GET; çalışan sunucu ister).
- Tek dosya `cargo test --test resp`; ada göre süzme `cargo test parse_ping`; çıktı için
  `-- --nocapture`. Doc testi yok.
- Sunucu isteyen yeni testleri `#[ignore = "requires a running ignix server on 127.0.0.1:7379"]`
  ile işaretle. `bash .hub/sunucu-testleri.sh` sunucuyu geçici bir dizinde başlatır ve
  `cargo test -- --include-ignored` koşar; argümanlar cargo test'e geçer:
  `bash .hub/sunucu-testleri.sh --test large_payloads`.

## Sunucuyu elle çalıştırma

- Önce port boş mu bak: `lsof -nP -iTCP:7379 -sTCP:LISTEN` (çıktı boş olmalı). Port sabittir;
  SO_REUSEPORT yüzünden ikinci bir `ignix` hata vermeden aynı portu paylaşır ve bağlantıların
  hangi sürece gideceği belirsizleşir.
- `cargo run` (hızlı derleme) ya da `cargo run --release` (LTO, yavaş derlenir). Sunucu çalışma
  dizinine `ignix.aof` yazar (`.gitignore`'da) ve açılışta okumaz. Günlük için `RUST_LOG=debug`.
- Deneme: `cargo run --example client` ya da kuruluysa `redis-cli -p 7379 PING`.
- `benchmarks/run_tests.sh` ve `benchmarks/run_benchmarks.sh` `pkill -9 ignix` çalıştırır ve
  makinedeki tüm ignix süreçlerini öldürür; paylaşılan makinede kullanma.

## Sandbox'ta cargo

Komutlar agy sandbox'ında çalışıyorsa çalışma ağacına yazılamaz ve ağ kapalıdır; cargo ise
`target/` dizinine yazar. `CARGO_TARGET_DIR="$TMPDIR/ignix-target" cargo check --offline
--all-targets` biçiminde dene (bağımlılıklar Hub'ın kurulum adımında `cargo fetch` ile iner).
Yine kalırsa aynı komutu tekrarlama; kodu kontrol edilebilir bırak, kanıtı Hub ayrı kopyada
üretir. `cargo fmt` dosya yazar; sandbox'ta `cargo fmt --check` çıktısındaki farkı dosya düzenleme
araçlarıyla uygula.

## Linux'a özgü kod (io_uring)

`src/net_uring.rs` ve Linux bağımlılıkları (`io-uring`, `libc`, `slab`) macOS'ta hiç derlenmez.
`cargo check --target x86_64-unknown-linux-gnu` da macOS'ta kalır: `libmimalloc-sys` C çapraz
derleyicisi (`x86_64-linux-gnu-gcc`) ister. Bu dosyayı değiştirdiysen Linux'ta (Linux makine, CI
ya da Docker'daki `rust` imajı) `cargo check --all-targets` ve
`cargo run --release -- --backend=uring` ile dene; deneyemediysen özetinde açıkça yaz.
