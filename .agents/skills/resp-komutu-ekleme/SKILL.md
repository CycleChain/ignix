---
name: resp-komutu-ekleme
description: Ignix'e yeni bir Redis komutu ekleme ya da mevcut bir komutun davranışını değiştirme. Cmd varyantı, parse_one argüman denetimi, Shard::exec yanıtı, Dict yöntemi, AOF kaydı, testler ve README/CHANGELOG güncellemesi adım adım; bilinen Redis farkları listesiyle.
---

# RESP komutu ekleme ya da değiştirme

Ignix'in ana genişletme noktası komut kümesidir. Bir komut dört katmana dokunur; ağ arka uçları
(`src/net.rs`, `src/net_uring.rs`) komut bilmez, değişmez.

## 1. Redis davranışını çıkar

`https://redis.io/docs/latest/commands/<komut>/` sayfasından şunları not et: argüman sayısı ve
seçenekler, dönüş tipi (durum, hata, tamsayı, toplu metin, null, dizi), hata mesajları, kenar
durumları (olmayan anahtar, yanlış tip, taşma). Kapsamı karta göre daralt: seçeneklerin hepsi
isteniyor mu, yalnızca temel biçim mi? Desteklemediğin seçeneği sessizce yok sayma; hata döndür
ya da ölçütte açıkça yaz.

## 2. `src/protocol.rs`

- `Cmd` enum'una doc yorumlu (İngilizce) bir varyant ekle; argümanlar `bytes::Bytes`
  (çoklu argüman için `Vec<Bytes>`, çiftler için `Vec<(Bytes, Bytes)>`).
- `parse_one` içindeki `if / else if` zincirine, son `else`'ten önce bir dal ekle:
  `items[0].eq_ignore_ascii_case(b"STRLEN") && items.len() == 2`. Argüman sayısını Redis'e göre
  denetle; bugün yanlış sayı "unknown/invalid command" hatasına düşer.
- Ayrıştırıcıda ayırmayı artırma; `items[i].clone()` (`Bytes`, referans sayacı) yeterlidir.

## 3. `src/storage.rs` (gerekirse)

Yeni bir depolama işlemi gerekiyorsa `Dict`'e `#[inline]` bir yöntem ekle. Oku-değiştir-yaz
işlemlerini `DashMap` entry API'siyle atomik yap (`incr` deseni); `get` ardından `set` yarış
yaratır.

## 4. `src/shard.rs`

- `Shard::exec` içindeki `match cmd`'ye kol ekle (eşleşme kapsamlıdır; kol eksikse derlenmez).
- Yanıtı `write_simple`, `write_bulk`, `write_null`, `write_integer`, `write_array_len` ile
  doğrudan `out`'a yaz. `resp_*` kullanma.
- Hata yanıtı RESP hata tipiyle (`-ERR ...\r\n`) gitmeli. `protocol.rs`'de hata yazıcısı yok;
  gerekiyorsa `write_simple` desenine uygun bir `write_error` ekle. Mevcut `+ERR` kullanımlarını
  kart istemedikçe değiştirme (davranış değişikliği olur).

## 5. `src/aof.rs` (veriyi değiştiren komutlar)

`emit_aof_<komut>` ekle ve `exec` içinde çağır: SET yazımdan önce, RENAME yalnızca başarıda,
INCR yürütmeden sonra kaydeder; komutun anlamına uygun olanı seç. Başarısız işlemi AOF'a yazma.

## 6. Testler

`tests/` altına yaz (Hub'ın regresyon denetimi yalnızca test yollarını temel sürüme taşır).
`tests/basic.rs` eski `exec` imzasını kullanır, derlenmez; örnek alma. Güncel desen:

```rust
use bytes::{Bytes, BytesMut};
use ignix::*;

fn run(shard: &Shard, cmd: Cmd) -> Vec<u8> {
    let mut out = BytesMut::new();
    shard.exec(cmd, &mut out);
    out.to_vec()
}

#[test]
fn rename_moves_value_to_new_key() {
    let shard = Shard::new(0, None);
    run(&shard, Cmd::Set(Bytes::from_static(b"a"), Bytes::from_static(b"1")));
    let reply = run(&shard, Cmd::Rename(Bytes::from_static(b"a"), Bytes::from_static(b"b")));
    assert_eq!(reply, b"+OK\r\n");
    assert_eq!(run(&shard, Cmd::Get(Bytes::from_static(b"b"))), b"$1\r\n1\r\n");
}

#[test]
fn get_with_wrong_arity_is_rejected() {
    let mut buf = BytesMut::from(&b"*1\r\n$3\r\nGET\r\n"[..]);
    let mut cmds = Vec::new();
    assert!(protocol::parse_many(&mut buf, &mut cmds).is_err());
}
```

Şunları sına: başarılı yol, olmayan anahtar, yanlış argüman sayısı (ayrıştırıcı), yanlış tip ya
da taşma (varsa), AOF kodlaması (`emit_aof_<komut>` çıktısı beklenen RESP baytlarına eşit).
Ağ üzerinden görünen davranış için `regresyon-testi` skill'indeki `#[ignore]` kuralına uy.

## 7. Belgeler

- README "Supported Commands" tablosu (komut, açıklama, örnek) ve `examples/README.md`
  "Supported Operations" listesi.
- CHANGELOG: en üstte `## [Unreleased]` başlığı altında `### Added` ya da `### Changed`
  (başlık yoksa ekle).
- Sık kullanılacak bir komutsa `benches/` altına criterion ölçümü eklemeyi öner
  (`performans-olcumu`).

## 8. Elle doğrulama (isteğe bağlı)

Sunucu açıkken `redis-cli -p 7379 STRLEN k`; kurulu bir Redis varsa aynı komutu
`redis-cli -p 6379` ile karşılaştır. Port ve süreç kuralları `derleme-ve-test` skill'inde.

## Bilinen Redis farkları (bugünkü `main`)

| Konu | Ignix | Redis |
|---|---|---|
| Hata yanıtı | `+ERR ...` (durum metni); RENAME ve ayrıştırma hataları | `-ERR ...` (hata tipi) |
| Bilinmeyen komut ya da ayrıştırma hatası | hatalı baytlar tamponda kalır; bağlantıdaki sonraki istekler de hata alır | bilinmeyen komutta `-ERR unknown command ...`, bağlantı sürer; bozuk protokolde hata yazıp bağlantıyı kapatır |
| DEL, EXISTS | yalnızca ilk anahtar; fazlası yok sayılır | çoklu anahtar, sayı döner |
| SET | EX, PX, NX, XX, GET seçenekleri yok sayılır | seçenekler desteklenir |
| INCR, tamsayı olmayan değer | değeri 1 yapar | `-ERR value is not an integer or out of range` |
| RENAME, aynı kaynak ve hedef | anahtar olmasa da `+OK` | anahtar yoksa hata |
| AOF | DEL yazılmaz; açılışta geri yüklenmez; UTF-8 olmayan veri bozulur | tam kalıcılık |
| Komut kümesi | PING GET SET DEL EXISTS INCR RENAME MGET MSET | yüzlerce komut (CLIENT, INFO, SELECT, HELLO...) |

Bu farkları kart istemedikçe düzeltme; dokunduğun komutta karşına çıkarsa özetinde belirt.
