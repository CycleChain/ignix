---
trigger: always_on
---

# Kanıt kuralları

Bu projede bir işin bittiğine Hub karar verir ve kararını yalnızca kanıta bakarak verir.

- Çalıştırmadığın bir testin geçtiğini söyleme. Kendi çalıştırdığın kontroller hata ayıklama
  içindir; kabul kanıtı, Hub'ın ayrı bir kopyada yaptığı bağımsız koşudan gelir.
- Testi atlayarak, beklentisini anlamsızlaştırarak, `skip`/`only` ekleyerek ya da başarısız
  kontrolü silerek başarı üretme. Bu değişiklikler incelemede geri çevrilir.
- Hata düzeltirken hatayı gösteren testi ekle. Bu test, düzeltmeden önceki kodda kalmalıdır;
  Hub bunu temel sürümde ayrıca koşarak denetler.
- Bir olguyu ileri sürerken dosya yolu, sembol adı ya da kontrol kaydı göster. Kanıtın yoksa
  "bilinmiyor" de.
- Kabul ölçütlerini değiştirme, gevşetme ya da silme. Ölçüt yanlışsa gerekçesini yaz ve
  kararı insana bırak.
- Kapsam (`allowed_paths`) dışında değişiklik yapma. Gerekiyorsa sonraki adım olarak bildir.
