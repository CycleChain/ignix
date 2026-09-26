---
name: regresyon-testi
description: Bu repoda bir hatayı gösteren regresyon testinin nasıl yazılacağı ve hatanın önce nasıl yeniden üretileceği.
---

# Regresyon testi yazma

Bir hata düzeltmeye başlamadan önce hatayı gösteren testi yaz. Sıra şudur:

1. Hatanın yeniden üretim koşulunu kartın açıklamasından çıkar.
2. Testi `tests/` altındaki ilgili dosyaya ekle; adı davranışı anlatsın (yer ve adlandırma
   kuralları aşağıda).
3. Testi düzeltmeden önce çalıştır ve **kaldığını** gör. Kalmıyorsa test yanlış yerdedir ya
   da hatayı tetiklemiyordur; düzeltmeye geçme.
4. Düzeltmeyi yap ve testin geçtiğini gör.

Hub, regresyon olarak işaretlenmiş ölçütlerde testi ayrıca temel sürümde koşar. Test orada
da geçiyorsa kanıt kabul edilmez.

## Bu repoda

- Regresyon testini `tests/` altına yaz (mevcut bir dosyaya ya da yeni `tests/<konu>.rs`).
  Hub temel sürüm koşusunda yalnızca `tests/` gibi test yollarındaki değişen dosyaları eski koda
  taşır; `src/` içindeki `#[cfg(test)]` modüllerine eklenen test temel sürümde hiç çalışmaz ve
  kanıt sayılmaz.
- Test adı davranışı anlatsın; Rust'ta adlar İngilizce ve snake_case:
  `rename_missing_key_returns_resp_error` gibi.
- Komut düzeyindeki hata için `Shard::exec`'i doğrudan çağır, yanıtı bir `BytesMut`'a yazdırıp
  baytları karşılaştır (örnek: `resp-komutu-ekleme` skill'i). Ayrıştırıcı hatası için
  `protocol::parse_many` ile `tests/resp.rs` desenini kullan.
- Hata yalnızca ağ üzerinden görünüyorsa (parçalı okuma, bağlantı durumu, büyük yük)
  `tests/large_payloads.rs` desenini kullan ve testi
  `#[ignore = "requires a running ignix server on 127.0.0.1:7379"]` ile işaretle. Böyle bir ölçütü
  `sunucu` kontrol profiline bağla; `test` profili sunucu başlatmaz.
- Çalıştırma: tek dosya `cargo test --test resp`, ada göre süzme `cargo test rename_missing`.
  Ayrıntılar `derleme-ve-test` skill'inde.
