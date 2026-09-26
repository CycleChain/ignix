@AGENTS.md

## Claude Code notları

- Skill'ler `.claude/skills` bağlantısıyla `.agents/skills/` klasöründen gelir.
- agy'nin her zaman yüklediği kurallar burada da geçerlidir:
  @.agents/rules/hub-contract.md
  @.agents/rules/kanit-kurallari.md
- Dosyaya özgü kurallar (agy `trigger: glob`) Claude Code'da kendiliğinden yüklenmez; ilgili
  dosyada çalışmadan önce oku: `.agents/rules/ag-arka-uclari.md` (`src/net.rs`,
  `src/net_uring.rs`, `src/bin/ignix.rs`) ve `.agents/rules/komut-semantigi.md`
  (`src/protocol.rs`, `src/shard.rs`, `src/storage.rs`, `src/aof.rs`).
- `.agents/agents/` altındaki alt ajan tanımları agy biçimindedir; Claude Code'da benzer bir iş
  için alt ajan açarken ilgili dosyanın `# Core Instructions` bölümünü istem olarak kullan.
