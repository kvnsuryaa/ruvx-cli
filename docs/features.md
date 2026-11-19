# Fitur ruvx

Dokumen ini merangkum kemampuan yang tersedia di ruvx v1 berdasarkan implementasi dan panduan penggunaan. ruvx adalah coding agent terminal berbasis Rust untuk macOS dan Linux.

## Menjalankan aplikasi

- Jalankan `ruvx [path]` untuk membuka workspace interaktif; path boleh berupa direktori kosong untuk proyek baru. Tanpa path, direktori kerja saat ini digunakan.
- Gunakan `ruvx --resume <session-id>` untuk melanjutkan sesi tersimpan. `--help` dan `--version` dapat dijalankan tanpa TUI.
- Antarmuka utama adalah TUI fullscreen; diperlukan terminal interaktif.

## Provider, akun, dan model

- **OpenAI API:** koneksi menggunakan API key tersimpan atau nama environment variable.
- **ChatGPT:** login browser melalui jalur resmi Sign in with ChatGPT, dengan pemilihan model yang tersedia bagi akun dan tunduk pada kelayakan akun/layanan.
- **Endpoint compatible:** konfigurasi base URL, kredensial, model ID, serta dukungan tools dan streaming. HTTP tanpa TLS hanya diizinkan untuk loopback lokal.
- Pilih koneksi dengan `/connect`, model dengan `/model`, dan reautentikasi profil ChatGPT dengan `/connect <profile>`.
- Model terakhir yang berhasil dipilih menjadi default global untuk sesi baru; sesi yang di-resume mempertahankan model sesi.
- Reasoning effort dapat dipilih melalui `/effort` atau kontrol TUI dan disimpan per profil/model. Daftar level dapat disesuaikan untuk model custom.

## Agent dan kontrol tindakan

- **Plan:** membaca instruksi `AGENTS.md`, melihat file, mencari teks, dan membaca file; tidak mengedit atau menjalankan shell.
- **Build:** dapat mengubah file melalui patch dan meminta persetujuan sebelum menjalankan perintah shell.
- Tools file membatasi akses ke workspace dan mengecualikan data privat aplikasi. Shell yang disetujui berjalan dengan izin OS pengguna; workspace bukan sandbox OS.
- Review perubahan sesi tersedia melalui `/diff [path]`, termasuk untuk workspace tanpa Git.
- Batas default adalah 30 putaran model per pekerjaan. `/continue` memulai budget baru; `agent.max_turns = 0` menghilangkan batas jumlah putaran.

## Percakapan dan TUI

- Chat fullscreen dengan streaming respons, riwayat yang dapat digulir, review, seleksi teks, serta salin melalui clipboard sistem atau OSC52.
- Command tersedia melalui `/help` dan menu command. Shortcut membantu navigasi, fokus kontrol, pengiriman pesan, dan perpindahan Plan/Build saat idle.
- Lampirkan file secara eksplisit dengan `@path` atau `@"path dengan spasi"`.
- Paste panjang ditampilkan sebagai node ringkas dan dapat diedit sebelum dikirim. Editor mendukung simpan, batal, dan hapus node; teks lengkap dikirim, bukan preview.
- Pesan user panjang dapat dibuka/tutup. Detail hasil tool tersedia terpisah.
- Aplikasi memulihkan terminal saat keluar atau pekerjaan dibatalkan. Shell panjang ditinjau sebelum persetujuan; pilihan awal adalah Deny.

## Sesi dan konteks

- Sesi dan percakapan disimpan lokal dan dapat dilanjutkan dengan `/sessions`, `/resume <id>`, atau opsi CLI.
- `/new` membuat sesi baru. `/compact` membuat ringkasan konteks secara manual tanpa menghapus riwayat asli.
- Konteks yang terlalu besar tidak dipotong otomatis; pemeriksaan konteks yang gagal mempertahankan draft. Tidak ada replay otomatis atas request atau tool saat resume.
- Satu folder proyek hanya dapat dibuka oleh satu proses ruvx aktif.

## Konfigurasi dan privasi

- Konfigurasi berada di `$XDG_CONFIG_HOME/ruvx/config.toml` (fallback `~/.config/ruvx/config.toml`); data sesi berada di `$XDG_DATA_HOME/ruvx/sessions/` (fallback `~/.local/share/ruvx/sessions/`).
- Kredensial tersimpan pada file privat dengan permission 0600 dan tidak dicatat sebagai bagian config utama atau jurnal percakapan.
- ruvx tidak memuat `.env` atau konfigurasi eksekusi proyek secara otomatis, dan tidak membaca token dari aplikasi coding lain.
- Perintah shell memerlukan persetujuan, tetapi setelah disetujui menggunakan izin OS pengguna dan bukan sandbox terisolasi.

## Batas cakupan v1

- Mode headless/pipe tidak didukung; dibutuhkan terminal interaktif.
- MCP, skills, subagent, undo otomatis, dan Windows native belum termasuk cakupan v1.
- Login ChatGPT bergantung pada kelayakan akun dan akses paket/preview. Keberhasilan mock atau pengujian lokal bukan jaminan untuk semua akun.

## Dokumentasi terkait

- [Panduan penggunaan](../README.md)
- [Kontrak tools dan kontrol agent](ruvx-v1/05-agent-tools-and-controls.md)
- [Konteks dan sesi](ruvx-v1/06-context-and-sessions.md)
- [Konfigurasi dan penyimpanan](ruvx-v1/07-configuration-and-storage.md)
- [Provider dan autentikasi](ruvx-v1/04-providers-and-auth.md)
- [Verifikasi dan batas pengujian](validation-v1.md)
