# Ignix: ajan talimatları

Bu dosya, bu repoda çalışan kodlama ajanları (Antigravity CLI `agy`, Claude Code ve benzerleri)
için tek kaynaktır. Claude Code bunu `CLAUDE.md` üzerinden okur. Ayrıntılı yöntemler
`.agents/skills/` altındaki skill'lerde, dosyaya özgü kurallar `.agents/rules/` altındadır.

## Proje

Ignix, Rust ile yazılmış, Redis protokolü (RESP) uyumlu, bellek içi bir anahtar-değer
sunucusudur. `ignix` crate'i aynı pakette bir kütüphane (`src/lib.rs`) ve sunucu ikilisi
(`src/bin/ignix.rs`) içerir; sürüm `Cargo.toml`'dadır (0.3.2), lisans MIT, crates.io'da
yayımlanır. Veritabanı, dış servis ya da gizli yapılandırma yoktur; tek ortam değişkeni
`RUST_LOG`'dur.

Projenin iki sözü vardır: Redis istemcileriyle doğrudan çalışmak ve çok çekirdekte yüksek iş
hacmi. Uyumluluğu Redis belgesine göre, başarımı ölçümle doğrula; ikisini de tahminle değiştirme.

## Komutlar

| Amaç | Komut |
|---|---|
| Bağımlılıklar | `cargo fetch` |
| Derleme | `cargo build` (hızlı), `cargo build --release` (LTO, codegen-units=1; yavaş) |
| Sunucu | `cargo run --release`; `0.0.0.0:7379` dinler, çalışma dizinine `ignix.aof` yazar |
| io_uring arka ucu | `cargo run --release -- --backend=uring` (yalnızca Linux; başka yerde mio'ya düşer) |
| Ayrıntılı günlük | `RUST_LOG=debug cargo run --release` |
| Örnek istemci | `cargo run --example client` (sunucu açıkken) |
| Tüm hedefleri derle | `cargo check --all-targets` |
| Lint, biçim | `cargo clippy -- -D warnings`, `cargo fmt --check` |
| Sunucusuz testler | `cargo test -- --skip test_large_payload` |
| Sunucu isteyen testler | `bash .hub/sunucu-testleri.sh` (sunucuyu kendisi başlatır ve durdurur) |
| Mikro benchmark | `CRITERION_HOME=target/criterion cargo bench --bench resp -- --noplot` |

Bu komutların bugünkü sonuçları, sandbox'ta çalıştırma ve test düzeni: `derleme-ve-test`
skill'i. Aşağıdaki "Bilinen durum" bölümünü mutlaka oku: bugün bazı komutlar `main`'de kalıyor.

## Mimari

İstek akışı: TCP bağlantısı → bağlantının okuma tamponu (`BytesMut`) →
`protocol::parse_many` → `Cmd` → `Shard::exec(&self, cmd, &mut out)` → `Dict` (DashMap) ve
veri değiştiren komutlarda `AofHandle` → yanıt `write_*` ile doğrudan `out`'a → soket.

- `src/net.rs` (varsayılan arka uç, mio): `run_shard`, `available_parallelism()` kadar iş
  parçacığı açar. Her biri `bind_reuseport` (SO_REUSEPORT) ile aynı portu kendi dinleyicisiyle
  dinler ve kendi olay döngüsünü (`run_worker_loop`) çalıştırır. Komutlar olay döngüsünde satır
  içi yürütülür; ayrı iş havuzu yoktur.
- `src/net_uring.rs`: Linux'a özgü io_uring arka ucu (`#![cfg(target_os = "linux")]`); tek iş
  parçacığı, SO_REUSEPORT yok, `unsafe` SQE gönderimleri. `--backend=uring` ile seçilir.
- `src/protocol.rs`: `Cmd` ve `Value` enum'ları; RESP ayrıştırıcı (`parse_one`, `parse_many`,
  `read_decimal_line`); sıfır kopya yanıt yazıcıları (`write_simple`, `write_bulk`,
  `write_null`, `write_integer`, `write_array_len`); eski, `Vec<u8>` döndüren `resp_*`.
- `src/shard.rs`: `Shard { id, dict, aof }`, 64 bayta hizalı (`test_shard_alignment` sınar);
  komut semantiği `exec` içinde. Sunucuda tek bir `Arc<Shard>` paylaşılır.
- `src/storage.rs`: `Dict` = `DashMap<Bytes, Value>`; `get`, `set`, `del`, `rename`,
  `exists`, `incr` (entry API ile atomik).
- `src/aof.rs`: `spawn_aof_writer` (ayrı iş parçacığı, 4096 kapasiteli sınırlı kanal; yazma
  geldikçe en çok saniyede bir `flush` + `sync_data`) ve `emit_aof_*` kodlayıcıları.
- `src/lib.rs`: modüller, `pub use` yeniden dışa aktarımları ve `DEFAULT_ADDR`
  (`0.0.0.0:7379`). Bunlar crates.io'daki genel API'dir.
- `src/bin/ignix.rs`: giriş noktası; mimalloc global ayırıcı, `--backend=uring` argümanı, AOF
  (`ignix.aof` açılamazsa AOF'suz sürer).

Desteklenen komutlar: PING, GET, SET, DEL, EXISTS, INCR, RENAME, MGET, MSET.

## Dizin haritası

| Yol | İçerik |
|---|---|
| `src/` | kütüphane ve sunucu (yukarıda) |
| `tests/` | `basic.rs` (`Shard::exec`), `resp.rs` (ayrıştırıcı), `large_payloads.rs` (127.0.0.1:7379'daki çalışan sunucuya bağlanır) |
| `benches/` | criterion: `exec.rs`, `resp.rs` (`harness = false`) |
| `examples/` | `client.rs` (cargo örneği); Python ve Node.js istemcileri, `verify_connection.*` |
| `benchmarks/` | Redis'e karşı Python benchmark paketi: `run_all.py`, `scripts/`, `quick_benchmark.py`, `run_*.sh` |
| `CHANGELOG.md` | Keep a Changelog biçiminde sürüm notları |
| `README.md` | kullanıcı belgesi; komut ve başarım tabloları |
| `HOW_TO_VERIFY_CONNECTION.md` | istemcinin Redis yerine Ignix'e bağlandığını doğrulama |
| `publish.sh` | `cargo package` ve `cargo publish --dry-run` |
| `.agents/`, `.claude/skills` | ajan kuralları, skill'ler, alt ajanlar |
| `.hub/` | agy-hub ayarları (`project.yaml`) ve `sunucu-testleri.sh` |

## Bilinen durum

Bugünkü `main` için geçerlidir. Görevin konusu değilse düzeltmeye kalkma; seni etkiliyorsa
özetinde belirt.

- **Kırık hedefler:** `tests/basic.rs` ve `benches/exec.rs`, `Shard::exec`'in eski imzasını
  kullanıyor; `cargo check --all-targets`, `cargo test` ve `cargo bench` derlenmiyor. Ayrıca
  `cargo clippy -- -D warnings` 3 uyarıda, `cargo fmt --check` 11 dosyada kalıyor. Hub kapıları
  bu yüzden her kartta kalır; ilk kart bunları düzelten bakım kartı olmalıdır. Başka bir kartta
  kapı yalnızca bu nedenlerle kalıyorsa `needs_human` ile sor.
- **Ağ testleri sunucu ister:** `tests/large_payloads.rs` çalışan bir sunucuya bağlanır;
  `.hub/sunucu-testleri.sh` kullan.
- **Sabit port, paylaşılan port:** adres `DEFAULT_ADDR`'dır, bayrakla değişmez. SO_REUSEPORT
  yüzünden aynı makinedeki ikinci bir `ignix` hata vermeden aynı portu paylaşır; sunucu
  başlatmadan önce `lsof -nP -iTCP:7379 -sTCP:LISTEN` ile portun boş olduğunu doğrula.
  `benchmarks/run_*.sh` betikleri `pkill -9 ignix` çalıştırır; paylaşılan makinede kullanma.
- **Protokol hatası bağlantıyı kilitler:** bilinmeyen bir komut ya da bozuk RESP, okuma
  tamponunda kalır; aynı bağlantıdaki sonraki bütün istekler `+ERR unknown/invalid command`
  alır. Bağlanırken `CLIENT SETINFO` gibi komutlar gönderen istemciler (ör. redis-py 5.x)
  etkilenebilir.
- **Hata yanıtları durum metni:** hatalar RESP hata tipi (`-ERR`) yerine `+ERR ...` olarak
  gidiyor (`shard.rs` RENAME, `net.rs` ayrıştırma hatası). Diğer Redis farkları:
  `resp-komutu-ekleme` skill'i.
- **AOF yalnızca yazılır:** açılışta geri yüklenmez; `DEL` AOF'a yazılmaz; `emit_aof_*`
  değerleri `String::from_utf8_lossy` ile yazdığı için UTF-8 olmayan veri bozulur.
- **Linux'a özgü kod macOS'ta derlenmez:** `net_uring.rs` macOS'ta denetlenemez (çapraz
  denetim de `libmimalloc-sys` yüzünden kalır).
- **`.gitignore` tuzakları:** `*.txt`, `*.svg`, `*.log`, `*.aof` kalıpları dışlanır; bu
  uzantılarla eklenen yeni dosyalar commit'e girmez. `Cargo.lock` listede olduğu hâlde
  izlenir; bağımlılık değişince onu da commit'le.
- README'deki `benchmark_redis_vs_ignix.py` yok; gerçek betikler `benchmarks/` altında.

## Kodlama kuralları

- Rust 2021 ve rustfmt varsayılanları (repoda `rustfmt.toml` yok). Bakım işi dışında ilgisiz
  dosyaları toplu biçimleme; diff şişer, inceleme zorlaşır.
- Sıcak yol (ayrıştırma, `Shard::exec`, ağ döngüsü) ayırma yapmamalı: yanıtı `write_*` ile
  doğrudan `BytesMut`'a yaz; yeni kodda `resp_*` kullanma; `Bytes` klonu (referans sayacı)
  kopyadan ucuzdur; `to_vec()`, `to_string()`, `format!` yalnızca kaçınılmazsa.
- Anahtar ve değerler `bytes::Bytes`; depolamaya yalnızca `Dict` yöntemleriyle eriş.
- Hata yönetimi `anyhow` (`bail!`, `Result`). `use anyhow::*;` `Ok`/`Err` adlarını
  gölgeleyebilir; `net.rs` ve `aof.rs`'deki `use std::result::Result::{Ok, Err};` satırını koru.
- Ağ döngüsüne `unwrap`/`expect` ekleme: bir bağlantının hatası iş parçacığını düşürmemeli.
- `unsafe` yalnızca `net_uring.rs`'te var; her yeni `unsafe` bloğu `// SAFETY:` gerekçesi ister.
- Platforma özgü kod `#[cfg(target_os = "linux")]` arkasında; Linux bağımlılıkları
  `[target.'cfg(target_os = "linux")'.dependencies]` altında.
- Bağımlılık eklemeden önce iki kez düşün; eklersen gerekçesini yaz ve `Cargo.lock`'u
  güncelle. `[profile.release]` ayarlarını (lto, codegen-units=1, panic=abort, strip) değiştirme.
- `pub` öğeler ve `lib.rs` yeniden dışa aktarımları crates.io kullanıcılarını etkiler; genel
  API değişikliğini CHANGELOG'a yaz.
- Kod yorumları, doc yorumları (`///`, `/*! */`), test adları, README ve CHANGELOG İngilizce
  yazılır (mevcut üslup). Bu dosya ve `.agents/` içeriği Türkçedir.

## Test ve doğrulama

- Komut semantiği değişirse `Shard::exec` düzeyinde, ayrıştırıcı değişirse `parse_many`
  düzeyinde test ekle; örnek desen `resp-komutu-ekleme` skill'inde. Regresyon testleri
  `tests/` altına yazılır (`regresyon-testi`).
- Ağ davranışı (parçalı okuma, büyük yük, bağlantı durumu) için sunucu isteyen test yaz ve
  `#[ignore = "requires a running ignix server on 127.0.0.1:7379"]` ile işaretle.
- Başarım iddiası ölçüm ister: önce/sonra criterion karşılaştırması (`performans-olcumu`).
- İş bitince (bakım kartından sonra): `cargo check --all-targets`, `cargo clippy -- -D
  warnings`, `cargo fmt --check`, `cargo test -- --skip test_large_payload`; ağ koduna
  dokunduysan `bash .hub/sunucu-testleri.sh`.

## Commit, CHANGELOG ve sürüm

- Commit mesajları İngilizce ve Conventional Commits biçiminde: `feat: ...`, `fix: ...`,
  `perf: ...`, `refactor: ...`, `docs(readme): ...`, `chore: ...`. Özet satırı kısa ve emir
  kipinde.
- Kullanıcıya görünen her değişiklik CHANGELOG'a girer (Keep a Changelog, SemVer); sürümü
  belirlenmemiş maddeler en üstteki `## [Unreleased]` başlığına yazılır.
- Sürüm hazırlığı `surum-yayini` skill'indedir. `cargo publish`, `cargo login` ve etiket
  gönderimi insan işidir; token isteme, okuma, yazma.
- agy-hub görevlerinde commit, push ve branch işlemlerini Hub yapar (`.agents/rules/hub-contract.md`).

## Ajan yapılandırması

- Kurallar (`.agents/rules/`): `hub-contract.md` ve `kanit-kurallari.md` her zaman geçerli;
  `ag-arka-uclari.md` (`src/net.rs`, `src/net_uring.rs`, `src/bin/ignix.rs`) ve
  `komut-semantigi.md` (`src/protocol.rs`, `src/shard.rs`, `src/storage.rs`, `src/aof.rs`)
  bu dosyalarda çalışırken geçerli.
- Proje skill'leri: `derleme-ve-test`, `resp-komutu-ekleme`, `performans-olcumu`,
  `surum-yayini`. agy-hub adım skill'leri: `explore-card`, `plan-card`, `implement-card`,
  `review-card`, `kabul-kaniti`, `regresyon-testi`. Topluluk skill'leri (MIT): `rust-testing`,
  `rust-patterns`; genel rehberdir, çelişkide bu dosya ve proje skill'leri geçerlidir.
- Alt ajanlar (`.agents/agents/`): `resp-uyumluluk-denetcisi` (komut davranışını Redis
  belgesiyle karşılaştırır, salt okuma) ve `performans-olcumcusu` (criterion ölçümlerini
  bağımsız koşar ve karşılaştırır).
