# Produk dan Keputusan ruvx v1

ruvx membantu pengembang individu menyelesaikan pekerjaan coding dari terminal: memahami proyek, mendiskusikan rencana, menerapkan perubahan, menjalankan validasi yang disetujui, lalu meninjau hasil. Produk dioptimalkan untuk proses lokal yang ringan dan core yang kecil.

## Masalah dan posisi produk

Pengguna ingin mengendalikan coding agent sendiri tanpa harus membawa seluruh kemampuan IDE, server, sistem plugin, atau orkestrasi banyak agent. Prioritas awalnya adalah pengalaman coding harian yang utuh dengan instalasi dan resource lokal yang terkendali.

Referensi utama adalah [nca](https://github.com/madebyaris/native-cli-ai), dengan pembanding [Pi](https://pi.dev/) untuk pendekatan core agent minimal, serta [OpenCode](https://opencode.ai/docs/providers/) untuk pola koneksi provider dan metode autentikasi. Fitur ketiga produk bukan daftar yang harus disalin seluruhnya; lihat [catatan riset](10-research-and-sources.md).

Keberhasilan awal dinilai dari dua pekerjaan nyata: mengubah repo existing dan membuat proyek baru dalam direktori kosong. Penggunaan oleh tim, publikasi luas, dan integrasi otomasi tidak menjadi kebutuhan penerimaan awal.

## Log keputusan diskusi

Tabel berikut mencatat hasil diskusi, termasuk pilihan yang dikoreksi pengguna. Nomor keputusan dipakai sebagai rujukan stabil dalam dokumentasi.

| ID | Keputusan | Implikasi |
| --- | --- | --- |
| D01 | Pemakaian pribadi dahulu | Workflow pengguna sendiri menjadi baseline; onboarding publik dan kebijakan tim ditunda. |
| D02 | Agent coding lengkap | Agent membaca repo, mengedit beberapa file, dan menjalankan validasi melalui tools. |
| D03 | Lightweight berarti ringan di mesin | Startup, RAM, ukuran binary, dan jumlah komponen lokal menjadi sasaran utama. |
| D04 | Core sengaja kecil | Tools bawaan dan alur inti lebih dahulu; tambahan fitur memerlukan kebutuhan yang jelas. |
| D05 | macOS dan Linux | Keduanya masuk matriks dukungan; Windows native ditunda. |
| D06 | Akun langganan serta API key | Koneksi tidak dibatasi hanya pada API key. |
| D07 | Langganan ChatGPT menjadi prioritas | Login layanan langganan lain tidak diwajibkan untuk v1. |
| D08 | Kendali melalui Plan dan Build | Kebijakan tindakan menjadi bagian inti agent, bukan sekadar label UI. |
| D09 | TUI fullscreen berfokus chat | Satu tampilan fullscreen, keyboard/mouse, seleksi drag dan copy otomatis, tanpa sidebar permanen. |
| D10 | Plan hanya baca/cari | Tidak mengedit proyek dan tidak mengeksekusi shell; perpindahan ke Build dilakukan pengguna. |
| D11 | Build mengedit langsung, shell disetujui | Persetujuan per patch tidak diwajibkan; persetujuan per shell command tetap wajib. |
| D12 | Core dibangun sendiri | nca dan OpenCode menjadi referensi, bukan runtime wajib atau fork dasar. |
| D13 | Rust | Core dan UI berada dalam satu binary native. |
| D14 | ChatGPT, OpenAI API, endpoint kompatibel | Protokol native Anthropic dan Gemini ditunda; akses via gateway kompatibel bergantung pada kapabilitas endpoint. |
| D15 | Target resource moderat | Startup ≤500 ms, RAM idle ≤100 MB, binary ≤40 MB pada kondisi ukur yang dicatat. |
| D16 | Autosave dan resume lokal | Riwayat tidak bergantung pada penyimpanan percakapan provider atau sinkronisasi cloud. |
| D17 | Konteks sesuai kebutuhan | List/search/read, AGENTS.md, dan referensi file; tanpa indeks embedding di awal. |
| D18 | Tools bawaan dahulu | MCP, skills, plugin, dan subagent ditunda. |
| D19 | Repo existing dan proyek baru wajib | Kedua skenario harus lulus pengujian penerimaan. |
| D20 | Compaction manual | Tidak merangkum atau memotong riwayat otomatis saat konteks penuh. |
| D21 | Ringkasan dan diff hasil | Checkpoint, undo otomatis, dan workflow commit khusus ditunda. |
| D22 | Sesi baru mulai di Plan | Mode terlihat jelas di terminal; resume memulihkan mode yang tersimpan. |
| D23 | 30 putaran default; 0 tanpa batas | Konfigurasi dapat mengubah batas tanpa mengubah aturan persetujuan shell. |
| D24 | Interaktif saja untuk v1 | Headless, one-shot, keluaran otomasi JSON, daemon, dan server ditunda. |
| D25 | Pola koneksi menyerupai OpenCode | `/connect` memilih provider lalu metode login yang didukung; auth terpisah dari inferensi. |
| D26 | Dokumentasi sebelum aplikasi | Folder rancangan ditulis dahulu. Setelah tahap tersebut selesai, pengguna meminta implementasi v1. |

Pilihan terdahulu yang sudah digantikan: pilihan inline terdahulu diganti fullscreen chat setelah diskusi lanjutan; API-key-only diperluas menjadi akun langganan dan API key; compaction otomatis ditolak; skenario existing-only diperluas ke dua skenario; target footprint lebih ketat tidak dipilih.

## Kebutuhan dan keberhasilan

| ID | Kebutuhan | Bukti penerimaan |
| --- | --- | --- |
| REQ-01 | Proses lokal ringan | Pengukuran build rilis pada macOS/Linux memenuhi D15. |
| REQ-02 | Mode benar-benar membatasi tindakan | Tool yang dilarang Plan tetap ditolak walau diminta langsung melalui event model. |
| REQ-03 | Siklus coding utuh | Agent dapat mengeksplorasi, mengedit, dan melaporkan hasil validasi. |
| REQ-04 | Multi-provider dan multi-auth | Jalur ChatGPT, OpenAI API, dan kompatibel memiliki pengujian terpisah. |
| REQ-05 | Konteks proyek tepat | AGENTS.md dan file relevan dapat dibaca tanpa memindai seluruh isi repo. |
| REQ-06 | Sesi dapat dilanjutkan | Restart memulihkan percakapan, rencana, mode, dan catatan tindakan. |
| REQ-07 | Pengguna mengendalikan compaction | Kapasitas konteks memicu jeda, bukan ringkasan atau pemotongan otomatis. |
| REQ-08 | Review hanya perubahan sesi | Diff tidak memasukkan perubahan yang sudah ada sebelum edit pertama sesi. |
| REQ-09 | Terminal tetap dapat digunakan | Streaming, Unicode, paste, resize, cancel, dan exit tidak merusak terminal. |
| REQ-10 | Kegagalan dapat dipahami | Error menyebut tahap, dampak, dan tindakan lanjut tanpa mengekspos kredensial. |
| REQ-11 | Run memiliki batas yang jelas | Default 30, unlimited 0, dan lanjut eksplisit bekerja konsisten. |
| REQ-12 | Dua workflow utama berhasil | Existing repo dan direktori proyek baru sama-sama lulus acceptance. |

Pemetaan lengkap skenario uji tersedia di [pengujian dan penerimaan](08-testing-and-acceptance.md).

## Batas v1

**Termasuk:** koneksi provider, pemilihan model, percakapan fullscreen, Plan/Build, tools file dan shell, instruksi proyek, referensi file, sesi lokal, compaction manual, ringkasan pekerjaan, dan diff.

**Ditunda:** distribusi untuk tim, Windows native, model inference yang dibundel, embedding/index service, MCP, skills, subagent, plugin pihak ketiga, background agent, cloud sync, undo/checkpoint otomatis, commit otomatis, serta antarmuka headless.

Server model lokal dapat dipakai jika menyediakan endpoint kompatibel yang memenuhi kontrak tools/streaming. ruvx tidak menginstal atau menjalankan inference server sebagai bagian v1.

## Default teknis yang mengikat rancangan

Rancangan memilih Ratatui/Crossterm untuk UI, runtime asinkron untuk network dan proses, TOML untuk konfigurasi, serta JSONL untuk jurnal sesi. Request Responses dan Chat Completions dinormalisasi ke event core yang sama. Pilihan ini mendukung D01–D26 tanpa menambah sistem plugin atau runtime agent eksternal.

Detail yang berasal dari rancangan teknis, seperti batas keluaran tool dan lokasi data, dijelaskan eksplisit di dokumen pemiliknya. Perubahan detail tersebut harus mempertahankan keputusan pengguna dan diperbarui konsisten pada dokumen pengujian.

## Definisi selesai produk

V1 selesai setelah seluruh REQ lulus, ketiga jalur koneksi diuji sesuai kelayakan akun, kedua workflow penerimaan berhasil, dan target resource diukur. Dokumentasi yang lengkap saja belum berarti aplikasi selesai. Tahap dokumentasi selesai ketika semua kontrak dan hubungan kebutuhan ke pengujian tercatat tanpa source code aplikasi.
