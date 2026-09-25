---
name: surum-yayini
description: Ignix'in yeni sürümünü hazırlama. Sürüm numarası (SemVer), Cargo.toml ve Cargo.lock, CHANGELOG (Keep a Changelog), README sürüm notları, cargo package ve cargo publish --dry-run denetimi. Yayın (cargo publish) ve etiket gönderimi insan işidir.
---

# Sürüm hazırlama

Ignix crates.io'da `ignix` adıyla yayımlanır. Ajan sürümü hazırlar ve denetler; `cargo publish`,
`cargo login` ve etiket gönderimini (`git push --tags`) insan yapar. Token isteme, okuma, yazma.

## 1. Durumu öğren

- Son etiket: `git tag --sort=-creatordate | head -3` (düzen `vX.Y.Z`; en eskisi `0.2.0`).
- Yayımlanmış son sürüm: `cargo search ignix --limit 1`.
- Son etiketten beri değişenler: `git log --oneline <son-etiket>..HEAD`.
- `Cargo.toml` içindeki `version` ile CHANGELOG'daki en üst sürümü karşılaştır. Bir sürüm
  CHANGELOG'da olup crates.io'da olmayabilir (0.3.1 böyle); bu normal.

## 2. Sürüm numarası

SemVer, 0.x kuralıyla: uyumsuz genel API değişikliği (`pub` öğeler, `Shard::exec` imzası gibi)
ya da büyük mimari değişiklik → küçük sürüm (0.3.2 → 0.4.0); hata düzeltmesi, başarım
iyileştirmesi, yeni komut → yama (0.3.2 → 0.3.3). Emin değilsen `needs_human` ile sor.

## 3. Dosyalar

1. `Cargo.toml` → `version = "X.Y.Z"`. Ardından `cargo check` çalıştır; `Cargo.lock` içindeki
   `ignix` girdisi güncellenir. `Cargo.lock` `.gitignore`'da listeli olduğu hâlde izlenir; iki
   dosya aynı commit'e girer.
2. `CHANGELOG.md`: en üste `## [X.Y.Z] - YYYY-MM-DD`. `## [Unreleased]` varsa maddelerini
   buraya taşı. Başlıklar mevcut üslupla İngilizce: `### Added`, `### Changed`, `### Fixed`,
   `### Performance`, `### Migration Notes`. Başarım maddesinde yalnızca ölçülmüş sayı yaz.
3. `README.md`: "Ignix vX.Y.Z architecture" satırı. "Benchmarks reflect Ignix vX.Y.Z" satırını
   ve tabloları yalnızca benchmark yeniden koşulduysa değiştir (`performans-olcumu`).

## 4. Denetim

```bash
cargo build --release
cargo test -- --skip test_large_payload     # sunucusuz testler; ayrıntı: derleme-ve-test
cargo package --list                        # pakete girecek dosyalar
cargo package                               # paketi kurar ve derler (temiz çalışma ağacı ister)
cargo publish --dry-run                     # publish.sh'deki denetim; yükleme yapmaz
```

`Cargo.toml`'da `include`/`exclude` yok; git'in izlediği her dosya pakete girer. Bugün pakete
`.agents/`, `.hub/`, `AGENTS.md`, `CLAUDE.md` ve `.claude/skills` bağlantısı izlenerek skill'lerin
ikinci bir kopyası da giriyor. Yayından önce `[package]` altına şu listenin eklenmesini insana
öner; `Cargo.toml`'u görev açıkça istemedikçe kendin değiştirme:

```toml
exclude = [".agents/", ".claude/", ".hub/", "AGENTS.md", "CLAUDE.md"]
```

`cargo package --list` çıktısında başka beklenmeyen dosya (büyük sonuç dosyaları,
`flamegraph.svg` gibi) varsa onu da bildir.

## 5. İnsana bırakılanlar

Özetinde şu komutları hazır ver, çalıştırma:

```bash
cargo publish
git tag vX.Y.Z
git push origin vX.Y.Z
```

Etiket, sürüm commit'ine konur (`v0.3.2` → db7bfd8 gibi). GitHub Release kullanılmıyor.
