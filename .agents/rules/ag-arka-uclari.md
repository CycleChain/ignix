---
trigger: glob
globs: "src/net.rs, src/net_uring.rs, src/bin/ignix.rs"
---

# Ağ arka uçları

İki arka uç aynı sözleşmeyi uygular: bağlantı başına okuma tamponu → `parse_many` → her `Cmd`
için `Shard::exec(cmd, &mut write_buf)` → yazma tamponunu sokete boşaltma. Komut semantiği
`Shard::exec` içindedir; arka uçlar komut bilmez.

- Bağlantı davranışını (hata yanıtı, bağlantıyı kapatma, tampon yönetimi) iki arka uçta birlikte
  değiştir. Yalnızca birini değiştiriyorsan nedenini özetinde yaz.
- `src/net.rs` (mio): her iş parçacığı kendi `Poll` döngüsünü ve `bind_reuseport` dinleyicisini
  çalıştırır; komutlar olay döngüsünde satır içi yürütülür. Döngüde bloklayan iş yapma (disk,
  `sleep`, uzun kilit); AOF yazımı kanal üzerinden ayrı iş parçacığına gider. Yazma tamponu
  boşalmadıysa `Interest::WRITABLE` ile yeniden kaydet (mevcut `reregister` deseni).
- G/Ç hataları: `would_block` WouldBlock ve Interrupted'ı ayırır; diğer hatalar yalnızca o
  bağlantıyı kapatır. Döngüye `unwrap`/`expect` ekleme; bir bağlantının hatası iş parçacığını
  düşürmemeli.
- Bilinen açıklar: `net.rs` ayrıştırma hatasında hatalı baytları okuma tamponunda bırakır; aynı
  bağlantıdaki sonraki istekler de hata alır. `net_uring.rs` ayrıştırma hatasını yanıtsız yutar ve
  kapanan bağlantının dosya tanıtıcısını kapatmaz (`libc::close` yorum satırında). Görev bunlarla
  ilgiliyse regresyon testiyle birlikte düzelt; değilse dokunma.
- `src/net_uring.rs` yalnızca Linux'ta derlenir (`#![cfg(target_os = "linux")]`); macOS'ta
  `cargo check` bu dosyayı hiç denetlemez. Burada değişiklik yaptıysan Linux'ta derlenip
  sınanmadığını özetinde açıkça belirt. `unsafe` SQE gönderimlerinde arabellek işaretçisinin
  işlem tamamlanana kadar geçerli kaldığını (`Box` içindeki okuma arabelleği, `Slab` girişi)
  koru ve her yeni `unsafe` bloğuna `// SAFETY:` gerekçesi yaz.
- `src/bin/ignix.rs`: argümanlar elle ayrıştırılır (yalnızca `--backend=uring`); adres
  `DEFAULT_ADDR` (`src/lib.rs`, `0.0.0.0:7379`). Yeni bayrak eklersen README'deki
  "Running the Server" bölümünü ve CHANGELOG'u güncelle.
- SO_REUSEPORT nedeniyle aynı makinede ikinci bir `ignix` süreci hata vermeden aynı portu
  paylaşır. Ağ testlerinden önce portun boş olduğunu doğrula (`derleme-ve-test` skill'i).
