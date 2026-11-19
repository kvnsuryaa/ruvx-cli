# Rancangan ruvx v1

ruvx adalah coding agent terminal yang ringan untuk penggunaan pribadi. Pengguna berdiskusi dan menyusun rencana dalam mode Plan, lalu berpindah ke Build untuk menerapkan perubahan. Core ditulis dalam Rust, memakai tools lokal, dan terhubung ke LLM melalui login ChatGPT atau API key.

Folder ini mencatat rancangan yang menjadi acuan implementasi v1. Isinya mencakup keputusan produk, kontrak perilaku, arsitektur, pengalaman terminal, serta rencana implementasi dan verifikasi. Aplikasi kini tersedia; panduan menjalankan ada di [README proyek](../../README.md), dan bukti pengujian ada di [laporan verifikasi](../validation-v1.md).

| Atribut | Nilai |
| --- | --- |
| Versi rancangan | v1 |
| Tanggal keputusan dan pemeriksaan sumber | 19 November 2026 |
| Bahasa dokumentasi | Indonesia; nama antarmuka teknis memakai bahasa Inggris |
| Nama produk / executable | ruvx / `ruvx` |
| Status | Implementasi tersedia; pengujian lokal dicatat terpisah dari pengujian akun nyata |
| Artefak | Source Rust, tests, script PTY, konfigurasi CI, dan dokumentasi |
| Pengguna awal | Pengembang individu; workflow pribadi menjadi tolok ukur |

## Cara membaca

Mulai dari produk dan keputusan, kemudian arsitektur dan UX. Dokumen provider, tools, konteks, serta penyimpanan merinci kontrak implementasi. Pengujian dan roadmap menerjemahkannya menjadi tahapan kerja yang dapat diperiksa.

| Dokumen | Pertanyaan yang dijawab |
| --- | --- |
| [01 Product and decisions](01-product-and-decisions.md) | Produk apa yang dibangun, mengapa, dan keputusan mana yang sudah dikunci? |
| [02 System architecture](02-system-architecture.md) | Bagaimana komponen dipisahkan dan bagaimana data mengalir? |
| [03 CLI and terminal UX](03-cli-and-terminal-ux.md) | Bagaimana pengguna menghubungkan provider dan mengerjakan tugas? |
| [04 Providers and auth](04-providers-and-auth.md) | Bagaimana API key, login akun, dan protokol provider dibedakan? |
| [05 Agent tools and controls](05-agent-tools-and-controls.md) | Tindakan apa yang dapat dilakukan agent dan kapan perlu persetujuan? |
| [06 Context and sessions](06-context-and-sessions.md) | Bagaimana konteks, riwayat, compaction manual, dan resume bekerja? |
| [07 Configuration and storage](07-configuration-and-storage.md) | Pengaturan dan data apa yang disimpan, di mana, dan dengan default apa? |
| [08 Testing and acceptance](08-testing-and-acceptance.md) | Bagaimana ketepatan perilaku dan sasaran performa dibuktikan? |
| [09 Implementation roadmap](09-implementation-roadmap.md) | Dalam urutan apa aplikasi nantinya dibangun? |
| [10 Research and sources](10-research-and-sources.md) | Sumber mana mendukung keputusan dan apa keterbatasan buktinya? |

## Ringkasan rancangan

- Satu binary Rust untuk macOS dan Linux; sesi interaktif memakai fullscreen chat dengan seleksi dan scrolling dalam aplikasi.
- Sesi baru dimulai dalam Plan. Hanya pengguna yang dapat memindahkannya ke Build.
- Plan menyediakan list/search/read. Build menambahkan patch dan shell; setiap shell command memerlukan persetujuan.
- Auth adapter mengelola identitas dan kredensial. Provider adapter mengelola request, streaming, dan tool calling.
- V1 menghubungkan ChatGPT melalui jalur resmi yang memenuhi syarat, OpenAI API, dan endpoint OpenAI-compatible.
- Riwayat tersimpan lokal dan dapat dilanjutkan. Compaction hanya dimulai oleh pengguna.
- Satu tugas dibatasi 30 putaran model secara default; nilai konfigurasi 0 berarti tanpa batas.
- Target build rilis: prompt siap paling lambat 500 ms, RAM idle maksimal 100 MB, binary maksimal 40 MB.

Target performa belum merupakan hasil pengukuran. Login, model discovery, respons LLM, dan proses shell memiliki metrik tersendiri.

## Status keputusan

**Keputusan pengguna** berasal dari diskusi dan dicatat di log keputusan. **Default teknis** merupakan pilihan konkret untuk menjalankan rancangan tersebut dan ditandai sebagai demikian di dokumen terkait. **Fakta sumber** disertai tautan primer. **Belum terverifikasi** menandai hal yang memerlukan implementasi atau pengujian nyata.

Jika ditemukan perubahan dokumentasi provider yang bertentangan dengan rancangan, perbarui catatan sumber dan keputusan adapter terkait terlebih dahulu. Jangan mengganti provider, metode login, atau aturan tindakan secara diam-diam.

## Glosarium

| Istilah | Arti di ruvx |
| --- | --- |
| Workspace | Direktori proyek kanonis yang dipilih saat memulai sesi; boleh merupakan direktori kosong. |
| Session | Riwayat percakapan, mode, pilihan model, dan tindakan yang terkait satu workspace. |
| Run | Pekerjaan agent yang dipicu satu pesan pengguna atau tindakan lanjut eksplisit. |
| Turn / putaran | Satu request generasi logis ke model; mencakup streaming responsnya. |
| Tool call | Permintaan tindakan terstruktur dari model yang divalidasi dispatcher sebelum dijalankan. |
| Auth adapter | Pengelola API key atau OAuth, masa berlaku, refresh, dan izin inference. |
| Provider adapter | Penerjemah konteks dan tools ke protokol provider serta hasil streaming ke event internal. |
| Compaction | Pembuatan ringkasan untuk konteks berikutnya tanpa menghapus riwayat asli. |
| Fullscreen TUI | Percakapan, input, review, scrolling, dan seleksi dikelola aplikasi pada alternate screen. |

## Riwayat tahap dokumentasi

Tahap awal hanya membuat dokumentasi sesuai permintaan pengguna. Setelah pengguna meminta implementasi v1, kontrak ini diterjemahkan menjadi aplikasi dan pengujian. Diagram serta mockup di folder ini tetap menjelaskan rancangan; status aktual, perubahan teknis berdasarkan pengujian, dan gate yang masih terbuka dilaporkan terpisah.

Panduan tambahan: [reasoning effort per model](../model-efforts.md). Preferensi effort kini dapat dipilih dari status TUI atau `/effort` dan disimpan per profil/model.

Panduan tambahan: [percakapan, paste panjang, dan preflight](../conversation-and-paste.md).
