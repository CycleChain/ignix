---
name: performans-olcumu
description: Ignix'te başarım ölçümü. criterion mikro benchmark'ları (temel çizgi kaydetme ve karşılaştırma), Redis'e karşı uçtan uca Python benchmark paketi, flamegraph ile profil ve sonuçların README/CHANGELOG'a yazılması. Başarım iddiası, optimizasyon ya da benchmark işi olduğunda kullan.
---

# Başarım ölçümü

Başarım bu projenin ana iddiasıdır. Ölçmeden "hızlandı" deme; önce ve sonra ölçümünü aynı
makinede, aynı koşullarda yap. Ölçümü bağımsız yaptırmak için `performans-olcumcusu` alt
ajanı vardır.

## Mikro benchmark'lar (criterion 0.5)

| Benchmark | Dosya | Ölçtüğü |
|---|---|---|
| `resp/parse_many_1k` | `benches/resp.rs` | 1000 SET komutluk tamponun `protocol::parse_many` ile ayrıştırılması |
| `exec/set_get` | `benches/exec.rs` | `Shard::exec` ile SET + GET döngüsü (bugün derlenmiyor: eski `exec` imzası) |

```bash
# değişiklikten önce (temel çizgi)
CRITERION_HOME=target/criterion cargo bench --bench resp -- --noplot --save-baseline temel
# değişiklikten sonra (karşılaştırma)
CRITERION_HOME=target/criterion cargo bench --bench resp -- --noplot --baseline temel
# tüm criterion hedefleri
CRITERION_HOME=target/criterion cargo bench --bench '*' -- --noplot
```

- `--noplot` yalnızca criterion hedeflerinde geçerlidir. `cargo bench -- --noplot` kütüphanenin
  libtest düzeneğinde "Unrecognized option: 'noplot'" ile kalır; hedefi `--bench` ile seç.
- `CRITERION_HOME` verilmezse criterion hedef dizinini bulmak için `cargo metadata` çalıştırır;
  başka bir cargo süreci `~/.cargo/.package-cache` kilidini tutuyorsa benchmark sessizce bekler.
- Sonuçlar ve HTML raporu `target/criterion/` altındadır (`report/index.html`).
- Karşılaştırmada criterion'un `change:` satırını (güven aralığı ve p değeri) ve "Performance has
  improved/regressed" ya da "No change in performance detected" yargısını olduğu gibi aktar.
- Gürültü büyüktür: paylaşılan bir makinede aynı kod art arda iki koşuda %24 "iyileşme"
  (p = 0.00) gösterdi. Başka yoğun iş yokken ölç, en az iki kez koş, tek koşuya dayanma.
- Yeni sıcak yol için benchmark: `benches/<ad>.rs` ve `Cargo.toml`'da
  `[[bench]] name = "<ad>"`, `harness = false`. Girdiyi döngü dışında hazırla, sonucu
  `black_box` ile tüket.

## Uçtan uca: Redis'e karşı (`benchmarks/`)

Gereksinimler: 6379'da Redis, 7379'da yayın derlemesi Ignix (`cargo build --release`, ardından
`./target/release/ignix`), Python 3. Grafik için `pip install matplotlib pandas seaborn numpy`;
yoksa grafikler atlanır.

- Hepsi: `cd benchmarks && python3 run_all.py` → `benchmarks/results/` altında `basic/`,
  `comprehensive/`, `real_world/` ve `index.html`. İki sunucu da açık değilse başlamaz.
- Tek tek: `python3 benchmarks/scripts/comprehensive_benchmark.py --target ignix --out <dizin>
  --json-out <dosya>`; `real_world_benchmark.py` aynı seçenekleri alır;
  `basic_benchmark.py --data-sizes 64 1024 --connections 1 10 --operations 1000
  --output-dir <dizin> --skip-plots`.
- Hızlı karşılaştırma: `python3 benchmarks/quick_benchmark.py`.
- README'deki `benchmark_redis_vs_ignix.py` betiği yok; yukarıdakileri kullan.
- `benchmarks/run_benchmarks.sh` (yalnızca `benchmarks/` içinden çalışır) ve `run_tests.sh`
  `pkill -9 ignix` kullanır; paylaşılan makinede çalıştırma. Portun boş olduğunu
  `lsof -nP -iTCP:7379 -sTCP:LISTEN` ile doğrula.
- Sonuç dizinleri ve dosyaları (`benchmarks/results/`, `benchmark_results.json`,
  `real_world_results.json`, `*.svg`, `*.txt`) `.gitignore`'dadır; commit'e girmez.

## Profil (flamegraph)

Yayın profili sembolleri siler (`strip = true`) ve hata ayıklama bilgisi içermez. Cargo.toml'u
değiştirmeden okunur yığın için:

```bash
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false cargo flamegraph --bin ignix
```

`cargo-flamegraph` kurulu olmalıdır; macOS'ta dtrace yönetici yetkisi ister (`--root`), bu adım
insan işidir. Kökteki `flamegraph.svg` eski bir çıktıdır; yeni `.svg` dosyaları `.gitignore`
yüzünden commit'e girmez.

## Sonuçları yazma

- README başarım tablolarını yalnızca gerçek koşu sonuçlarıyla güncelle; makineyi, işletim
  sistemini, Redis sürümünü, veri boyutlarını ve bağlantı sayısını yaz ve "Benchmarks reflect
  Ignix vX.Y.Z" satırını düzelt.
- CHANGELOG `### Performance` maddesi ölçülmüş sayıyı ve karşılaştırma koşulunu içerir.
- Hub'daki `bench` profili yalnızca benchmark'ların derlenip çalıştığını kanıtlar. Sayıyı
  özetinde ver; başarım ölçütü `manual: true` olur (`kabul-kaniti`).
