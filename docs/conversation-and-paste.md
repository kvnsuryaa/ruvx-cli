# Percakapan dan paste panjang

ruvx membedakan pesan **USER** dari jawaban **AI · ruvx** melalui label, background, padding, dan gaya teks. Satu baris kosong berada di bawah setiap label sebelum isi pesan; pada USER jarak ini mengikuti background pesan, sedangkan pada AI memakai background terminal. Jarak tampilan tidak masuk clipboard atau pesan model. Pesan user memakai latar netral dengan teks terang, tanpa bingkai atau gutter; AI memakai latar terminal dan area baca penuh agar kode tetap lebar. Warna bukan satu-satunya penanda. Saat bagian awal pesan sudah berada di luar layar, heading menyebut peran yang sedang dibaca sebagai `continued`.

Tools dan informasi sistem tampil lebih tenang. Saat tool berjalan, nama aktivitas tampil di status; hasilnya muncul sekali di percakapan, misalnya `read_file → ok  [Detail]`. Baris pembuka nama/ID tidak dicetak lagi. Call ID, argumen, dan hasil lengkap tetap tersedia melalui Detail pada run aktif. Resume menampilkan ID dan hasil dari jurnal dengan status yang tersimpan; untuk jurnal lama tanpa metadata status, label memakai `finished` dan status aktual tersedia saat Detail dibuka. Setiap pemanggilan berbeda tetap dicatat secara terpisah.

Usage bukan lagi JSON mentah di percakapan, melainkan baris seperti:

```text
Tokens · input 413 · output 14 · total 427  [Detail]
```

Detail membuka metadata provider asli. Nilai yang tidak dilaporkan tidak diisi nol; total diambil dari aggregate provider, tanpa menjumlahkan rincian `attribution` yang bisa merupakan bagian dari angka yang sama. Format Responses maupun Chat Completions didukung. Usage juga dipulihkan dari artefak respons saat resume.

## Paste sebelum dikirim

Paste dengan **≥8 baris logis atau ≥4.096 byte UTF-8** berubah menjadi node preview, misalnya:

```text
> Tolong periksa kode berikut: [Paste 1 · 120 lines] lalu jelaskan masalahnya.
```

Paste pendek tetap berupa teks biasa. Beberapa node paste dapat dicampur dengan teks ketikan, sesuai urutan. Preview tidak mengirim, memotong, atau merangkum konten. Enter mengirim isi lengkap dari semua node. Preview tidak mengurangi penggunaan token model.

| Interaksi | Perilaku |
| --- | --- |
| Klik preview atau F2 | Buka editor paste; F2 memilih node di kursor atau node terakhir |
| Ctrl+S / Save | Simpan pengeditan dan kembali ke composer |
| Escape / Cancel | Batalkan pengeditan; isi node sebelumnya tetap utuh |
| Ctrl+D / Remove di editor paste | Hapus node dari draft |
| Panah, Home/End, PgUp/PgDn, wheel | Navigasi/pengeditan pada tampilan teks per bagian |
| Ctrl+Home / Ctrl+End di editor paste | Ke awal/akhir dokumen |
| Backspace/Delete di composer | Node preview dihapus secara utuh; panah melewatinya sebagai satu node |

Editor paste menampilkan teks penuh di seluruh layar dengan pembacaan bertahap. Pengeditan memakai potongan referensi sumber dan buffer tambahan, sehingga tidak perlu menyalin ulang seluruh dokumen untuk setiap ketukan tombol. Save menghasilkan sumber baru; Cancel tidak mengubah sumber asli. Jika shell meminta approval saat editor sedang terbuka, editor ditangguhkan dan dilanjutkan sesudah review tanpa kehilangan perubahan yang belum disimpan.

Paste panjang diperlakukan sebagai bagian pesan user, termasuk bila dimulai `/quit` atau command lain. Command lokal tetap tersedia melalui teks ketikan biasa. Sintaks referensi `@path` yang sudah ada tetap berlaku pada pesan yang dirakit. Newline CRLF/CR dinormalisasi ke LF dan kontrol terminal disanitasi seperti input sebelumnya; kata, indentasi, serta isi pesan tidak dipotong diam-diam.

Input pada dialog kredensial/konfigurasi/approval tetap literal; tidak berubah menjadi node preview. API key tetap dimasking. Paste tidak menekan Enter atau memilih approval secara otomatis.

## Pesan yang sudah terkirim

Pesan user dengan **≥12 baris logis atau ≥4.096 byte** dilipat menjadi maksimal **enam baris visual**, disertai kontrol Open/Close. Jawaban AI tetap terbuka, termasuk kode panjang. Semua kontrol dapat diakses melalui klik atau F6/Shift+F6, Tab/arrows, lalu Enter.

Folding hanya state tampilan. Sumber lengkap tetap tersimpan dalam jurnal/artefak session; resume menerapkan kembali aturan folding tanpa migrasi format. Riwayat tampilan tetap terpisah dari konteks model, sehingga compaction manual tidak menghilangkan pesan lama dari UI.

Seleksi menggunakan offset sumber. Padding, label peran, tombol, serta isi yang masih tersembunyi tidak ikut tersalin. Buka pesan dahulu untuk menyeleksi seluruh isinya. Ketika pengguna membaca bagian lama, streaming tidak menarik layar ke bawah; Latest kembali mengikuti respons terbaru.

## Ukuran besar dan pemeriksaan konteks

Tidak ada batas ukuran draft tetap. Isi node disimpan pada file sementara privat, dibaca dalam halaman **32 KiB**, dan dibungkus dengan indeks jarang per **128 baris visual**. UI hanya memegang rentang teks yang diperlukan untuk frame; indeks dan penghitung tetap bertambah mengikuti banyaknya pesan/baris. Berkas sumber tidak mempertahankan satu file descriptor terbuka untuk setiap pesan.

Respons streaming ditambahkan ke sumber file, dan indeks diperbaiki mulai checkpoint terakhir yang berubah. Resize membangun indeks untuk lebar baru; mengetik di bagian awal editor tidak menghitung ulang seluruh ekor dokumen. Riwayat input terakhir berisi hingga 50 snapshot/referensi, sehingga tidak menyimpan 50 salinan penuh paste besar di RAM.

Enter memulai pemeriksaan pesan dan kapasitas konteks di worker, sementara draft tetap tersedia di composer. Draft memiliki revisi: perubahan isi atau membuka editor paste membatalkan pemeriksaan lama. Hasil revisi lama tidak boleh memulai request. Ctrl+C juga dapat membatalkannya; prioritas menyalin seleksi yang aktif tetap berlaku.

Jika preflight gagal, draft dan seluruh node tetap tersedia, tidak dicatat sebagai pesan terkirim, dan tidak ada inference yang dimulai. Untuk kapasitas yang terlampaui, kontrol **Compact context**, **New session**, dan editor tersedia tanpa perlu membuang draft. Compaction tetap eksplisit, memakai konteks lama tanpa menyertakan draft yang belum dikirim. Session baru tidak mengirim otomatis dan memakai model terakhir yang tersimpan secara global bila masih valid; draft tetap dipertahankan. Jika model belum tersedia, pemilihan model juga mempertahankan draft.

Jika batas model tidak diketahui, provider masih dapat menolak request. Pesan yang sudah melewati preflight tetap tersedia dalam riwayat input untuk dipulihkan, tanpa retry otomatis. Perhitungan konteks merupakan estimasi ukuran, bukan tokenizer billing yang pasti.

Tanpa batas tetap bukan berarti penggunaan memori nol: parser bracketed paste menerima payload utuh, persiapan request merakit teks lengkap, dan decoding artefak JSON dapat membutuhkan buffer sebanding ukuran artefak. Buffer sementara, metadata indeks, konteks aktif, ruang disk, dan kapasitas model tetap menjadi batas praktis. Batas copy clipboard yang sudah ada (16 MiB seleksi dan 100 KB untuk jalur OSC52) terpisah dari ukuran input.

## Verifikasi

Kasus uji mencakup pemisahan peran tanpa warna, usage ringkas/detail, folding dan copy sumber, Unicode sampai grapheme sangat panjang, node campuran, edit/simpan/batal/hapus, payload lengkap, konteks gagal, revisi/cancel preflight, draft saat ganti model/session, kredensial, serta approval yang menyela editor.

Stress tests Rust menggunakan paste **1 MiB**, **10 MiB**, dan transcript **10.000 pesan**. Ukuran tersebut adalah kasus pengujian, bukan batas produk. PTY memeriksa alur paste nyata, pengeditan sebelum kirim, payload mock HTTP, slash literal, token usage, dan regresi terminal. Benchmark tambahan mengukur transfer/preview paste, respons keyboard sesudahnya, serta RSS proses parent. Tidak ada inference provider nyata atau biaya LLM dalam pengujian.

## Hasil pengukuran

Penyederhanaan visual selanjutnya memakai background netral tanpa bingkai/gutter. Sebanyak 73 tes, format, Clippy, release build, dan PTY fungsional diuji ulang di macOS; PTY berwarna memverifikasi background/foreground dan copy teks user sesudah perubahan padding. Bukti fungsional: [background macOS](validation/background-macos-arm64.json). Pengaturan `NO_COLOR` tetap dihormati; label peran tetap terbaca saat warna dimatikan.

Penambahan jarak di bawah label dan menu slash command diverifikasi dengan **67 unit + 12 integration tests** pada macOS arm64 dan Linux aarch64. Formatting, Clippy macOS, dan release build kedua platform lulus. Uji PTY mencakup filtering `/m`, pengisian argumen, Tab, Escape/edit, klik bantuan, resize 48×16, serta jarak label pada percakapan nyata. Tes Rust mencakup wheel/drag, Shift+Tab, kondisi kosong, dialog/paste, dan larangan antrean command selama run aktif. Inspeksi frame 100×40 dan 48×16 serta output warna menu dilakukan di macOS, dengan `NO_COLOR` dihapus hanya pada fixture pemeriksaan warna. Tidak ada dependency baru atau pengukuran resource ulang. Bukti fungsional: [command macOS](validation/commands-macos-arm64.json), [command Linux](validation/commands-linux-arm64.json).

Pembersihan duplikasi aktivitas tool diverifikasi dengan **80 unit + 12 integration tests (92 total)**, release build, dan PTY pada macOS arm64 serta Linux aarch64; format dan Clippy lulus di macOS. Tes memeriksa satu hasil per pemanggilan, status error yang tetap terlihat, ID/rincian dalam Detail, resume dengan jurnal lama tanpa metadata status, serta pembacaan artefak sesuai kebutuhan. PTY memeriksa hasil shell tanpa baris nama/ID duplikat dan Detail yang masih memuat ID/argumen. Metadata status baru pada `tool.finished` bersifat tambahan; jurnal lama tetap dapat dibaca. Bukti: [tool macOS](validation/tool-results-macos-arm64.json), [tool Linux](validation/tool-results-linux-arm64.json).

Angka resource di bawah adalah baseline percakapan/paste sebelum penyederhanaan visual tersebut.

Verifikasi final pada **19 November 2026**, Rust **1.89.0**, versi aplikasi **0.1.0**:

- **61 unit tests + 12 integration tests (73 total)** lulus pada macOS arm64 dan Linux aarch64.
- Format dan Clippy seluruh target dengan `-D warnings` lulus di macOS. Build release dan acceptance PTY lulus pada kedua platform.
- Inspeksi frame PTY mencakup draft, editor, dan percakapan pada 100×40, 74×28, serta 48×16. Observer VT tidak menggantikan pemeriksaan langsung pada beberapa emulator desktop.
- Seluruh target startup, RSS idle, dan ukuran binary tetap terpenuhi. Tidak ada dependency tambahan.

| Metrik | macOS arm64 | Linux aarch64 | Target |
| --- | --- | --- | --- |
| Startup p95 | 57.818 ms | 61.346 ms | ≤500 ms |
| Maksimum RSS idle teramati | 5.931 MB | 5.206 MB | ≤100 MB |
| Executable release | 6.120 MB | 5.773 MB | ≤40 MB |

Uji bracketed paste nyata melalui PTY, dengan model fixture yang batas konteksnya diketahui:

| Ukuran | Transfer sampai preview: macOS / Linux | Respons keyboard: macOS / Linux | Maksimum RSS parent: macOS / Linux |
| --- | --- | --- | --- |
| 1 MiB | 164.9 / 86.2 ms | 55.3 / 51.4 ms | 11.731 / 7.303 MB |
| 10 MiB | 946.2 / 304.1 ms | 54.9 / 51.1 ms | 85.131 / 47.161 MB |

Pengujian 10 MiB juga membuktikan penolakan preflight konteks dengan node/draft tetap utuh dan tanpa event pesan user terkirim. Pengujian respons HTTP lengkap memakai pesan multiline yang muat dalam konteks fixture; stress 10 MiB bukan inference provider nyata.

Waktu transfer mencakup transport PTY, decoding bracketed paste, sanitasi, penyimpanan, dan render 30 FPS. Waktu keyboard adalah waktu deteksi harness sesudah transfer selesai, dengan pump sekitar 50 ms, sehingga bukan pengukuran latensi aplikasi presisi sub-milidetik. RSS disampel melalui `ps` setiap 50 ms dan dapat melewatkan peak singkat; tidak menghitung child processes atau page cache OS. MB memakai 1.000.000 byte, MiB 1.048.576 byte.

Startup memakai 20 sampel warm-cache pada PTY 100×40 dan config/session kecil; RSS idle diamati 10 detik tanpa discovery/inference atau child shell. Host: Apple M1 8 GiB, macOS 14.3 arm64; Linux: Debian Bookworm/glibc 2.36 aarch64 dalam Docker LinuxKit 6.12.5 pada host yang sama. Beban OS/VM tidak sepenuhnya diisolasi. Hasil bukan Linux bare metal/x86_64 atau jaminan memori untuk seluruh ukuran input.

Bukti mentah, seluruh sampel, profil file/streaming/snapshot/resume, serta SHA-256 binary: [macOS](validation/messages-macos-arm64.json), [Linux](validation/messages-linux-arm64.json). Login/inference akun nyata, penerimaan clipboard pada emulator desktop, dan seluruh fault injection tetap belum dinyatakan terverifikasi.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark --output target/validation-messages.json
```
