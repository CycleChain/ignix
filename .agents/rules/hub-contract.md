---
trigger: always_on
---

# Hub çalışma sözleşmesi

Bu repo agy-hub tarafından yönetilen otonom görevlerde de kullanılır. İstem bir Trello kart
bağlantısı içeriyorsa Hub'ın başlattığı bir çalışmadasın; şu kurallara uy:

- Yalnızca bulunduğun çalışma ağacında çalış. Üst dizinlere ya da başka repolara yazma.
- Commit atma, push yapma, branch değiştirme. Bunları Hub yapar.
- Yeni agent açma, iç içe agy oturumu başlatma. Rol dağıtımını Hub yürütür; keşif ayrı bir
  adımdır ve senin tarafından başlatılmaz.
- `.github/workflows/`, `.hub/` ve `.agents/` altındaki dosyaları görev açıkça istemedikçe
  değiştirme.
- Gizli bilgileri (token, parola, `.env` içeriği) okuma, yazma ya da çıktıya koyma.
- Yeni bağımlılık gerekiyorsa projenin paket yöneticisini ve kilit dosyasını kullan;
  gerekçesini özetinde belirt.
- Geri dönüşü zor bir tercihle karşılaşırsan (veri silme, genel API değişikliği, ürün kararı)
  dur ve `needs_human` alanına en fazla dört seçenekli kısa bir soru yaz.
- Bir komut reddedilirse aynı komutu tekrar deneme; başka bir yol bul ya da nedenini özetle.
- Sonucu her zaman rolünün JSON şemasında döndür.
