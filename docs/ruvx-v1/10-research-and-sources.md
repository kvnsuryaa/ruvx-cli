# Riset dan Sumber Rancangan ruvx v1

Sumber primer berikut mendukung identitas referensi, kelayakan integrasi provider, dan pilihan terminal. Pemeriksaan dilakukan dalam diskusi dan penyusunan dokumentasi pada **19 November 2026** melalui Firecrawl. Tanggal ini adalah tanggal pemeriksaan, bukan tanggal publikasi atau jaminan bahwa isi tidak akan berubah.

Pada pembuatan folder dokumentasi awal, belum ada benchmark ruvx, pengujian login, atau panggilan inference. Angka performa pada rancangan merupakan target produk. Hasil pengujian implementasi berikutnya tersedia dalam [laporan verifikasi](../validation-v1.md) dan dibedakan dari fakta sumber di bawah.

## Produk referensi

| ID | Sumber primer | Fakta yang dipakai | Dampak pada ruvx |
| --- | --- | --- | --- |
| SRC-01 | [nca README](https://github.com/madebyaris/native-cli-ai) | Referensi `nca-cli` cocok dengan native-cli-ai. README menjelaskan Rust, single binary, beberapa antarmuka terminal, multi-provider, dan fitur agent tambahan. | Rust/native dan pengalaman terminal menjadi referensi; daftar fiturnya tidak diadopsi seluruhnya. |
| SRC-02 | [nca speed and memory](https://github.com/madebyaris/native-cli-ai#speed-and-memory) | Penulis memberi konteks pada sampel footprint macOS dari satu mesin/build lama; bukan peak RAM universal. | Tidak memakai angka sampel nca sebagai hasil komparasi atau janji footprint ruvx. |
| SRC-03 | [nca provider docs](https://github.com/madebyaris/native-cli-ai/blob/main/docs/documentation/providers.md) | Dokumentasi menjelaskan beberapa provider dan endpoint kompatibel dengan kredensial API. | Membantu memisahkan dukungan banyak provider dari pertanyaan login subscription. |
| SRC-04 | [nca license](https://github.com/madebyaris/native-cli-ai/blob/main/LICENSE) | Lisensi MIT dengan kewajiban mempertahankan notice yang relevan untuk reuse. | Opsi fork pernah dipertimbangkan; keputusan akhir core sendiri. Tidak ada kode nca disalin pada tahap ini. |
| SRC-05 | [Pi](https://pi.dev/) | Memposisikan diri sebagai minimal agent harness dengan kemampuan terminal dan ekstensi. | Menjadi referensi untuk memisahkan core kecil dan kemampuan tambahan. |
| SRC-06 | [OpenCode providers](https://opencode.ai/docs/providers/) | Provider dapat memiliki metode login berbeda. OpenAI menyediakan sign-in ChatGPT; custom endpoint memiliki konfigurasi tersendiri. | Pola `/connect` memilih provider, auth, lalu model. |
| SRC-07 | [OpenCode provider account methods](https://opencode.ai/docs/cli/providers/#methods) | Dokumentasi akun membedakan metode autentikasi seperti API key dan OAuth. | Auth adapter dipisahkan dari provider adapter; akun langganan tidak disamakan dengan format API key. |

OpenCode bukan dependensi runtime ruvx. Mencontoh alur koneksinya tidak berarti menyalin client ID, token, endpoint privat, schema penyimpanan, atau seluruh sistem pluginnya. Perilaku provider yang didukung tetap harus diperiksa terhadap dokumentasi layanan masing-masing.

## OpenAI dan autentikasi

| ID | Sumber primer | Dukungan yang relevan | Pemakaian dalam rancangan |
| --- | --- | --- | --- |
| SRC-08 | [SIWC plan usage overview](https://developers.openai.com/siwc/token-sharing-open-source) | Jalur akses paket untuk aplikasi open-source/lokal yang memenuhi syarat. | Dasar kelayakan direct account login; tidak dianggap universal untuk semua akun/produk. |
| SRC-09 | [Cookbook Sign in with ChatGPT](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt) | Contoh penggunaan jalur resmi termasuk proyek personal lokal. | Core sendiri tetap mungkin tanpa runtime Codex sebagai kewajiban. |
| SRC-10 | [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in) | Registrasi dinamis, callback loopback, PKCE, validasi identitas/scope, dan protected local storage. | Rujukan implementasi OAuth dan pengujian callback. |
| SRC-11 | [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions) | Pemisahan registrasi/identitas serta lifecycle kredensial dan refresh. | Record terpisah dan refresh yang diserialkan. |
| SRC-12 | [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) | Model serta route inference publik untuk akses paket. | Discovery terkait akun dan penggunaan Responses yang didukung. |
| SRC-13 | [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations) | Streaming/store/history serta batas payload/tools berbeda dari request API biasa. | Capability terpisah untuk auth subscription; jangan hanya mengganti bearer token. |
| SRC-14 | [OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling#streaming) | Tool calls dan fragmen argumen streaming perlu dirakit menurut protokol. | Normalisasi call identity dan eksekusi sesudah completion/validasi. |

Sumber SIWC registrasi, preview limitations, dan OpenCode providers dibaca kembali saat penulisan folder. Sumber primer lain telah dibaca pada tahap riset diskusi; hal teknis yang berubah wajib diperiksa ulang saat implementasi dimulai.

### Perbedaan penting

- Identitas OAuth yang valid tidak otomatis memberi izin inference paket.
- Subscription adalah model akses/tagihan; metode kredensialnya dapat berbeda per layanan.
- Jalur subscription dan API-key dapat memakai keluarga protokol yang sama dengan parameter/capability berbeda.
- Keberhasilan implementasi login produk lain tidak membuktikan bahwa kredensial atau jalurnya boleh dipakai sembarang aplikasi.
- Ketentuan akses paket dan preview adalah fakta temporal; uji akun aktual tetap diperlukan.

Rancangan ruvx memilih jalur direct yang didokumentasikan untuk kebutuhannya. Opsi menjalankan Codex app-server pernah diperiksa, tetapi tidak dipilih karena core sendiri merupakan keputusan produk. Rujukan alternatif tersebut tersedia pada [Codex app-server](https://developers.openai.com/codex/app-server#auth-endpoints).

## Terminal dan Rust

Versi berikut adalah versi dokumentasi yang diteliti, bukan dependency lock yang sudah diinstal.

| ID | Sumber primer | Fakta yang dipakai | Konsekuensi desain |
| --- | --- | --- | --- |
| SRC-15 | [Ratatui 0.30.2 initialization](https://docs.rs/ratatui/0.30.2/ratatui/fn.try_init_with_options.html) | Inline viewport dapat diinisialisasi tanpa alternate screen. | Pertahankan terminal scrollback. |
| SRC-16 | [Ratatui Terminal](https://docs.rs/ratatui/0.30.2/ratatui/struct.Terminal.html#inline-viewport) | Inline viewport dan `insert_before` mendukung konten di atas area live. | Pisahkan stable transcript dan live tail; commit konten sekali. |
| SRC-17 | [Ratatui resize](https://docs.rs/ratatui/0.30.2/ratatui/struct.Terminal.html#method.resize) | Resize terminal bukan kontrak setter tinggi composer inline dinamis. | V1 memakai tinggi live area tetap dan input internal scroll. |
| SRC-18 | [Crossterm 0.29.0 events](https://docs.rs/crossterm/0.29.0/crossterm/event/) | Pembacaan event tidak boleh dicampur antara EventStream dan reader sinkron. | Implementasi memakai poll/read pada task UI yang sama dengan rendering; lihat temuan PTY pada laporan verifikasi. |
| SRC-19 | [Crossterm terminal raw mode](https://docs.rs/crossterm/0.29.0/crossterm/terminal/#raw-mode) | Raw mode mengubah pemrosesan input terminal biasa. | Tangani Ctrl+C dan terminal cleanup secara eksplisit. |
| SRC-20 | [Unicode segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/) | Grapheme segmentation mendukung operasi teks Unicode. | Editor tidak bergerak berdasarkan byte string. |

Tokio, Reqwest/Rustls, TOML, dan JSONL adalah pilihan teknis untuk arsitektur. Rancangan tidak mengklaim kombinasi tersebut sudah memenuhi target resource; dependency features dan versi perlu dipin pada implementasi dan hasilnya diukur.

## Pemisahan fakta dan keputusan

| Pernyataan | Status |
| --- | --- |
| nca adalah referensi yang dimaksud dalam diskusi | Identitas dikonfirmasi melalui repo/situs primer. |
| OpenCode tidak dibatasi hanya pada API key | Fakta dokumentasi provider. |
| Direct ChatGPT plan usage tersedia untuk integrasi yang memenuhi syarat | Fakta dokumentasi temporal; eligibility aktual belum diuji untuk ruvx. |
| Rust, fullscreen chat UI (revisi setelah inline), Plan/Build, API key dan subscription, core kecil | Keputusan pengguna. |
| TOML, JSONL, satu renderer, adapter protokol terpisah | Rancangan teknis yang dipilih. |
| Snapshot file mendukung review session | Rancangan; coverage dan atribusi harus diuji. |
| Startup ≤500 ms, RAM idle ≤100 MB, binary ≤40 MB | Target penerimaan, belum hasil pengukuran. |
| ruvx berhasil coding end-to-end atau login live | Workflow lokal sudah diuji dengan mock; login dan inference akun nyata belum dibuktikan. |

## Pemeriksaan sebelum implementasi provider

Periksa kembali eligibility, nama scope, callback, issued client registration, endpoint, bentuk tools, parameter yang ditolak, daftar model akun, dan refresh behavior. Catat tanggal serta bagian sumber yang berubah. Perubahan yang memengaruhi produk masuk log keputusan; perubahan encoding protokol masuk kontrak adapter dan fixtures.

Jika sumber tidak dapat dibaca atau saling bertentangan, tandai bagian spesifik sebagai belum terverifikasi dan jangan menggantinya dengan endpoint hasil tebakan. Informasi komunitas dapat membantu discovery, tetapi sumber resmi menjadi acuan klaim dukungan integrasi.

## Keterbatasan riset

Riset ini cukup untuk merancang v1, bukan evaluasi menyeluruh semua coding agent atau semua provider. Tidak ada klaim bahwa ruvx lebih cepat atau lebih hemat daripada produk referensi. Tidak ada penilaian kualitas model, biaya per tugas, atau kompatibilitas seluruh OpenAI-compatible servers.

Dokumentasi provider yang baru diambil dapat tetap memiliki batas preview dan akses akun. Timestamp fetch tidak mengubah contoh satu mesin menjadi benchmark independen dan tidak membuktikan ketersediaan fitur pada akun tertentu.

## Revisi fullscreen

Keputusan UI terbaru menggantikan implikasi inline pada SRC-15/16/17. Referensi tersebut tetap mencatat riset awal; implementasi sekarang memakai fullscreen, transcript virtual, dan wrapping ulang. Pola drag seleksi, copy saat mouse dilepas, dan prioritas Ctrl+C mengikuti [dokumentasi fullscreen Claude Code](https://code.claude.com/docs/en/fullscreen#use-the-mouse), dengan clipboard helper OS serta fallback OSC52. Detail kontrak ada pada [UX terminal](03-cli-and-terminal-ux.md).
