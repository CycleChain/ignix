# İstem şablonları (isteğe bağlı)

Bu klasöre `plan.md`, `implement.md`, `continue.md` ya da `review.md` koyarsan worker
kendi varsayılanı yerine seninkini kullanır. Varsayılanlar agy-hub deposunda
`worker/prompts/` altındadır; başlangıç için oradan kopyala.

Yer tutucular `{{ad}}` biçimindedir:

| Yer tutucu | İçerik |
|---|---|
| `{{card_title}}`, `{{card_url}}`, `{{card_body}}` | Trello kartı (meta bloğu çıkarılmış açıklama) |
| `{{plan}}` | Onaylı planın Markdown hâli |
| `{{feedback}}` | İnsan cevabı, revize notu, hakem gerekçeleri ya da kapı raporu |
| `{{attempt}}` | Geliştirme deneme sayısı |
| `{{gates}}`, `{{diffstat}}`, `{{diff}}` | Yalnızca `review.md` içinde |

`continue.md`, aynı konuşmaya geri bildirimle devam edilirken kullanılır.
