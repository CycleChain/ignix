---
name: kabul-kaniti
description: Kabul ölçütlerinin nasıl yazılacağı ve hangi kanıtın hangi ölçüte bağlanacağı.
---

# Kabul ölçütü ve kanıt

Kabul ölçütü, dışarıdan gözlenebilir tek bir davranışı anlatır ve nasıl sınanacağını söyler.

İyi: `K1: Olmayan anahtar için RENAME, RESP hata yanıtı (-ERR no such key) döner (test profili: test).`
Kötü: `RENAME düzeltilir.`

Kurallar:

- Her ölçüte kısa bir kimlik ver: K1, K2...
- Otomatik sınanabiliyorsa `check_profile_id` alanına projedeki bir kontrol profilini bağla.
- Hatayı gösteren test ise `regression: true` yaz.
- Yalnızca insan bakınca anlaşılıyorsa (görsel düzen, metin tonu) `manual: true` yaz;
  bu ölçüt son onayda insana gösterilir.
- Ölçüt sayısını küçük tut: işin gerçekten neyi karşılaması gerektiğini anlatan üç beş madde.
- Bu repoda profiller `.hub/project.yaml` içindedir: `test` (sunucusuz testler), `sunucu`
  (sunucuyu başlatıp tüm testleri koşar), `bench` (criterion). Ağ davranışını anlatan ölçütü
  `sunucu` profiline bağla.
- Başarım ölçütü (ör. `resp/parse_many_1k` en az %10 hızlanır) otomatik kanıt üretmez: `bench`
  profili yalnızca benchmark'ın çalıştığını gösterir. Sayıyı özetle birlikte ver ve ölçütü
  `manual: true` yap.

Doğrulayıcı her ölçüt için `supported`, `unsupported` ya da `unknown` verir. Kanıtı olmayan
bir ölçüt `unknown` olur ve iş doğrulanmaz.
