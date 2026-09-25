# Skill'ler

Her alt dizin bir `SKILL.md` taşır (`name` ve `description` ön bilgisi zorunlu). agy yalnızca
adları ve açıklamaları bağlama alır; içerik gerektiğinde ya da `/ad` ile çağrıldığında yüklenir.
Claude Code aynı klasörü `.claude/skills` bağlantısı üzerinden görür.

- `explore-card`, `plan-card`, `implement-card`, `review-card`: agy-hub adımları. Worker bu
  adımların istemini `/explore-card ...` biçiminde başlatır; skill repoda yoksa istem yine
  çalışır, yalnızca bu rol talimatları eklenmez. İş akışları (`.agents/workflows/`) agy'de
  kullanımdan kalktı. agy 1.2.11 `.agents/agents/` tanımlarını da okur, ama Hub rolleri bu
  skill'lerle verir; worker `--agent` bayrağını yalnızca `agy agents` listesindeki adlar için
  kullanır.
- `kabul-kaniti`, `regresyon-testi`: kabul ölçütü ve regresyon testi kuralları, bu repoya
  uyarlandı.
- Proje skill'leri: `derleme-ve-test` (komutlar, test düzeni, bilinen kırıklar),
  `resp-komutu-ekleme` (ana genişletme noktası, bilinen Redis farkları), `performans-olcumu`
  (criterion, Redis karşılaştırması, flamegraph), `surum-yayini` (sürüm hazırlığı).
- Topluluk skill'leri: `rust-testing`, `rust-patterns` (MIT; kaynak ön bilgideki
  `metadata.source` alanında, lisans metni klasördeki `LICENSE` dosyasında). Genel rehberdir;
  çelişkide `AGENTS.md` ve proje skill'leri geçerlidir.

İyi bir skill: dar bir konuyu, bu repodaki gerçek komutlar ve dosya yollarıyla anlatır.
