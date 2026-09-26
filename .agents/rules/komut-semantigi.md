---
trigger: glob
globs: "src/protocol.rs, src/shard.rs, src/storage.rs, src/aof.rs"
---

# Komut semantiği ve RESP uyumluluğu

- Bir komutun davranışı dört yerde tanımlıdır: `Cmd` varyantı ve argüman denetimi
  (`protocol::parse_one`), yürütme ve yanıt (`Shard::exec`), depolama (`Dict`), kalıcılık
  (`emit_aof_*`). Birini değiştirirken diğerlerini de denetle; adım adım liste
  `resp-komutu-ekleme` skill'indedir.
- Yanıt tipi Redis ile aynı olmalı: durum `+OK`, hata `-ERR ...`, tamsayı `:`, toplu metin `$`,
  yok `$-1`, dizi `*`. Emin değilsen `https://redis.io/docs/latest/commands/<komut>/` belgesine
  bak; tahmin etme. Mevcut kod hataları `write_simple("ERR ...")` ile `+ERR` olarak gönderiyor;
  yeni kodda bu deseni çoğaltma.
- Veriyi değiştiren her komut AOF'a yazılır (bugün `DEL` yazılmıyor; bilinen açık).
- `parse_one` `Ok(None)` ile "daha fazla veri gerek", `Err` ile protokol hatası bildirir. Eksik
  veriyi hata sayma; parçalı TCP okumaları normaldir.
- Sıcak yol ayırma yapmamalı: yanıtları `write_*` ile doğrudan `BytesMut`'a yaz, `Bytes`
  klonlarını (referans sayacı) kopyaya tercih et; `to_vec()`, `to_string()` ve `format!` yalnızca
  kaçınılmazsa.
- `resp_*` kodlayıcıları (`Vec<u8>` döndürür) kütüphanenin genel API'sidir (`pub use
  protocol::*`); yeni kodda kullanma, silme de.
- `Shard` üzerindeki `#[repr(align(64))]` ve `test_shard_alignment` testini koru.
- Davranış değişince README "Supported Commands" tablosunu, `examples/README.md` işlem
  listesini ve CHANGELOG'u güncelle.
