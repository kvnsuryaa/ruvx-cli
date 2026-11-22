# ruvx

Coding agent terminal berbasis Rust untuk macOS dan Linux. Mulai dengan **Plan** untuk membaca dan memahami proyek, lalu pilih **Build** untuk mengedit. Setiap perintah shell meminta persetujuan pengguna.

ruvx memakai core agent sendiri, TUI fullscreen berfokus pada chat, dan koneksi provider melalui API key atau akun ChatGPT yang memenuhi syarat.

Proyek ini sebelumnya bernama Kovr. Command sekarang `ruvx` dan folder sumber `ruvx-cli`. Config, kredensial, session, pilihan model, dan cache selalu memakai direktori XDG bernama `ruvx`, terpisah dari direktori `kovr`. Data Kovr tetap utuh dan tidak dibaca atau disalin otomatis; hubungkan provider kembali melalui `/connect` pada config baru. Lokasi default dijelaskan di [panduan penyimpanan](docs/ruvx-v1/07-configuration-and-storage.md). Sesi tetap terkait path workspace kanonisnya; setelah folder proyek dipindah/di-rename, mulai sesi baru pada path baru. Arsip verifikasi lama mempertahankan nama/path/hash binary yang diuji saat itu. Verifikasi perubahan nama sebelum pemisahan penyimpanan: [macOS](docs/validation/rename-ruvx-macos-arm64.json), [Linux](docs/validation/rename-ruvx-linux-arm64.json).

## Instalasi dari release

Unduh paket yang sesuai dengan OS/arsitektur dari [GitHub Releases](https://github.com/kvnsuryaa/ruvx-cli/releases), beserta `SHA256SUMS`. Binary siap pakai tidak memerlukan Rust. Target yang disiapkan: macOS Apple Silicon/Intel dan Linux ARM64/x86_64.

Contoh untuk macOS Apple Silicon versi `0.1.0`, setelah release dipublikasikan:

```sh
shasum -a 256 --check --ignore-missing SHA256SUMS
tar -xzf ruvx-v0.1.0-aarch64-apple-darwin.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 ruvx-v0.1.0-aarch64-apple-darwin/ruvx "$HOME/.local/bin/ruvx"
export PATH="$HOME/.local/bin:$PATH"
ruvx --version
ruvx /path/ke/proyek
```

Jalankan verifikasi checksum di direktori unduhan dan pastikan paket yang dipilih mendapat status `OK` sebelum ekstraksi. Di Linux gunakan `sha256sum --check --ignore-missing SHA256SUMS`. Simpan pengaturan PATH di konfigurasi shell agar tersedia pada terminal berikutnya. Paket Linux dibangun pada Ubuntu 22.04/glibc 2.35; distro lebih lama dan musl belum diverifikasi. Pengujian paket macOS memakai macOS 15. Binary macOS belum ditandatangani dengan Developer ID atau dinotariskan.

Untuk maintainer, proses tag, build empat target, dan publikasi draft dijelaskan di [panduan release](docs/releasing.md).

## Menjalankan dari source

Gunakan Rust 1.89 atau lebih baru:

```sh
cargo build --release --locked
./target/release/ruvx /path/ke/proyek
```

Direktori harus sudah ada. Direktori kosong dapat digunakan untuk proyek baru. Tanpa argumen path, ruvx memakai direktori kerja saat ini.

Opsional, instal binary ke direktori Cargo pengguna:

```sh
cargo install --path . --locked
ruvx /path/ke/proyek
```

`--help` dan `--version` bekerja tanpa terminal interaktif. Mode pipe/headless belum tersedia.

## Menghubungkan provider

Di dalam ruvx, jalankan `/connect` dan ikuti pilihan:

1. **OpenAI API:** gunakan API key tersimpan atau nama environment variable.
2. **Continue with ChatGPT:** login di browser, selesaikan consent, lalu pilih model dari daftar akun. ruvx menggunakan jalur resmi Sign in with ChatGPT dan memvalidasi identitas serta izin inference.
3. **Custom compatible:** tentukan API base URL, metode kredensial, model ID, dan dukungan tools/streaming. HTTP tanpa TLS hanya tersedia pada loopback lokal.

Setelah koneksi native selesai memuat model, gunakan `/model`. Model tertentu juga dapat dipilih secara eksplisit dengan `/model <profile> <model-id>`. Reauthorization akun ChatGPT tersedia melalui `/connect <profile>`.

Model terakhir yang berhasil dipilih otomatis menjadi default global pengguna. Sesi baru, `/new`, dan window baru pada proyek lain langsung memakai profil/model tersebut beserta effort yang tersimpan. Pilihan pada window yang sudah berjalan tetap dipertahankan; resume memakai model milik sesi lama. Setelah pembaruan ini, pilih model sekali untuk mulai menyimpan default global. Satu folder proyek tetap hanya dapat dibuka oleh satu proses ruvx aktif.

Login ChatGPT mengikuti kelayakan akun serta batas akses paket/preview layanan. Implementasi lokal dan mock bukan bukti keberhasilan pada semua akun; status pengujiannya dicatat di [laporan verifikasi](docs/validation-v1.md).

Kredensial tersimpan di file privat 0600 dan tidak menjadi bagian config utama atau jurnal percakapan. ruvx tidak membaca token dari aplikasi coding lain.

## Percakapan dan paste

Pesan **USER** memakai background netral tanpa bingkai; jawaban **AI · ruvx** tetap lebar untuk kode. Satu baris kosong memisahkan label peran dari isi pesan. Baris di atas input menampilkan nama model dan mode aktif. Pesan user panjang dapat dibuka/tutup, sementara jawaban AI tetap terbuka. Usage tampil ringkas dengan Detail terpisah.

Hasil tool tampil sekali, misalnya `read_file → ok [Detail]`; aktivitas berjalan ada di status dan call ID tersedia dalam Detail.

Saat AI bekerja, indikator di bawah percakapan, tepat di atas area input, hanya menampilkan spinner dan empat informasi: `AI Working, 12s, 2 turns, 1530 total tokens`. Durasi, jumlah turn, dan total token berlaku untuk run aktif dan direset pada run berikutnya, termasuk `/continue`. Total token menjumlahkan usage setiap respons provider; jika `total_tokens` tidak ada, jumlah input dan output yang lengkap digunakan. Sebelum usage tersedia, atau bila ada turn tanpa usage lengkap, tampil `— total tokens`; angka ini bukan estimasi konteks. Indikator tetap bergerak ketika provider atau tool belum menghasilkan output. Menunggu persetujuan memakai label `Waiting for approval`, dan pembatalan memakai `Canceling`, dengan metrik ringkas yang sama. Petunjuk approval ada di dialog; `Ctrl+C cancel` tetap tersedia di footer. Setelah selesai atau gagal, spinner berhenti dan status kembali `ready`. Di editor paste, indikator juga berada di bawah, tepat di atas footer.

Jika proyek berada di repository Git, kanan baris status menampilkan branch seperti `git: main`, dengan pembaruan otomatis sekitar dua detik. Worktree dan subfolder repository didukung; detached HEAD menampilkan hash pendek. Informasi ini dibaca dari metadata lokal tanpa menjalankan proses Git.

Paste panjang muncul sebagai `[Paste 1 · N lines]`. Klik atau F2 untuk mengedit, Ctrl+S menyimpan, Escape membatalkan, dan Ctrl+D menghapus node di editor. Enter mengirim isi lengkap, bukan teks preview. Draft tidak memiliki batas ukuran tetap; pemeriksaan konteks gagal mempertahankan draft, dengan kontrol compaction/session baru yang tetap eksplisit.

Lihat [panduan percakapan dan paste](docs/conversation-and-paste.md) untuk batas praktis, penyimpanan, serta hasil pengujian.

## Reasoning effort

Jalankan `/effort` atau klik **[Effort: …]** untuk memilih tingkat reasoning model aktif. Pilihan tersimpan per profil/model dan muncul kembali saat berpindah model atau membuka aplikasi lagi.

```text
/effort high
/effort default
```

Menu hanya menampilkan level yang diketahui didukung. Untuk gateway/model custom yang belum memiliki metadata, tetapkan daftar sesuai dokumentasi providernya, misalnya `/effort levels low medium high`. `default` tidak mengirim parameter effort; `none` meminta no reasoning secara eksplisit bila model mendukungnya. Perubahan berlaku saat idle dan tidak menjalankan tugas.

Lihat [panduan effort per model](docs/model-efforts.md) untuk konfigurasi, daftar preset, format provider, dan pengujian.

## Workflow

- **Plan:** list/search/read serta pembacaan AGENTS.md, tanpa shell atau edit proyek.
- **Build:** `/mode build`, kemudian kirim instruksi menerapkan pekerjaan. Patch berjalan langsung; shell menunggu review dan pilihan `run` atau `deny`.
- **Review:** `/diff` memperlihatkan perubahan yang dicatat session, termasuk proyek tanpa Git. Baseline memakai kondisi sebelum perubahan session, sehingga perubahan pengguna sebelumnya tetap terpisah.
- **Kelanjutan:** autosave lokal; `/sessions` dan `/resume <id>` melanjutkan tanpa replay tindakan. Dari CLI tersedia `ruvx --resume <id>`.
- **Konteks panjang:** `/compact` merangkum secara manual sambil menyimpan riwayat asli. ruvx tidak memotong atau merangkum history otomatis.
- **Run panjang:** default 30 putaran; `/continue` membuka budget baru setelah batas tercapai. `agent.max_turns = 0` menghilangkan batas jumlah putaran.

Tools file membatasi path ke workspace dan mengecualikan data privat aplikasi. Shell yang disetujui berjalan dengan izin OS pengguna; cwd bukan sandbox OS. Review shell mencakup teks workspace yang disnapshot dan menandai cakupan yang dikecualikan.

## Command dan input

Ketik `/` untuk membuka menu command di atas input; `/m` memfilter `/model`, `/mcp`, dan `/mode`. Gunakan Up/Down atau wheel di atas menu untuk memilih, Enter/klik untuk menjalankan atau membuka pilihan, Tab untuk melengkapi tanpa menjalankan, dan Escape untuk menutup tanpa menghapus input. `/mode` dan `/resume` dilengkapi dengan spasi agar argumennya dapat diisi. Menu tidak mengeksekusi command selama AI bekerja; pilih lagi setelah selesai. Command yang tersedia sama dengan `/help`.

| Command/input | Kegunaan |
| --- | --- |
| `/help` | Daftar command dan shortcut |
| `/connect`, `/disconnect`, `/model` | Provider, kredensial, dan model |
| `/effort [default\|level]`, `/effort levels [...]` | Reasoning effort dan kapabilitas model aktif |
| `/skills [add\|remove\|enable\|disable]` | Daftar skill dan pengelolaan lokasi skill lokal |
| `/mcp`, `/mcp add`, `/mcp edit <id>`, `/mcp remove <id>`, `/mcp enable <id>`, `/mcp disable <id>` | Daftar dan konfigurasi MCP stdio tanpa menjalankan server |
| `/mode plan`, `/mode build` | Berpindah mode ketika idle |
| `/new`, `/sessions`, `/resume <id>` | Pengelolaan session |
| `/compact` | Ringkasan konteks manual melalui model |
| `/diff [path]` | Review perubahan session |
| `/continue` | Lanjut setelah batas putaran |
| `/acknowledge` | Setelah memeriksa efek tindakan terputus, tandai outcome yang belum pasti tanpa mengeksekusi ulang |
| `/quit` | Keluar dan pulihkan terminal |
| Enter / Alt+Enter | Periksa dan kirim teks lengkap / tambah baris |
| Option+Delete / Ctrl+Backspace / Ctrl+W | Hapus kata sebelumnya, termasuk whitespace di belakangnya |
| Ctrl+U | Hapus dari kursor ke awal baris logis; baris lain dipertahankan |
| Klik preview / F2 | Edit blok paste panjang |
| Ctrl+S / Escape / Ctrl+D di editor paste | Simpan / batalkan edit / hapus blok |
| Drag mouse | Seleksi teks percakapan/review; otomatis salin saat mouse dilepas |
| Ctrl+C | Salin dan bersihkan seleksi; tanpa seleksi, batalkan pekerjaan aktif; idle: kosongkan draft lalu keluar |
| Wheel / PgUp / PgDn | Gulir percakapan atau review |
| Alt+End / klik Latest | Kembali mengikuti pesan terbaru |
| Shift+Tab | Berganti Plan/Build saat idle; draft tidak dikirim |
| F6 / Shift+F6, Tab/arrows, Enter | Fokus kontrol berikut/sebelumnya, navigasi, lalu aktifkan; Escape kembali ke input |
| Escape | Bersihkan seleksi terlebih dahulu; berikutnya tutup review/dialog |
| `@path`, `@"path dengan spasi"` | Lampirkan file secara eksplisit |
| Awalan `//` | Kirim teks yang dimulai `/` |

Riwayat digulir di dalam aplikasi dan tetap tersedia sesudah `/compact` serta resume. Saat membaca bagian lama, streaming tidak menarik layar ke bawah. Klik `[Detail]` untuk hasil tool; `/diff` dan `/help` membuka review yang dapat digulir. Pada review shell panjang, baca sampai bagian bawah sebelum menyetujui; pilihan awal selalu **Deny**.

Shortcut hapus kata/baris juga tersedia di editor paste. Option perlu diteruskan oleh terminal; bila tombol ditahan terminal, gunakan Ctrl+W/Ctrl+U atau atur pemetaannya. ruvx meminta pelaporan modifier melalui [keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/#modifiers) pada terminal yang mendukungnya.

Clipboard memakai `pbcopy` di macOS, atau helper `wl-copy` (Wayland), `xclip`/`xsel` (X11) yang sudah tersedia di Linux. SSH atau helper yang tidak tersedia memakai OSC52; terminal harus mengizinkan akses clipboard. Status “Copy sent to terminal” berarti permintaan terkirim, bukan konfirmasi clipboard lokal. Jalur OSC52 dibatasi 100 KB, dan satu seleksi maksimal 16 MiB. ruvx tidak memasang helper otomatis. Isi yang tersembunyi dan bingkai UI tidak ikut tersalin.

`ui.viewport_rows` dari konfigurasi lama tetap diterima tetapi diabaikan oleh fullscreen. `ui.max_fps` tetap membatasi redraw (default 30). Tidak ada fallback inline.

AGENTS.md dibaca menurut scope direktori; instruksi baru/berubah harus masuk konteks sebelum agent mengedit target. Instruksi file dan respons model tidak dapat mengubah mode atau melewati approval.

## Konfigurasi lokal

Config menggunakan `$XDG_CONFIG_HOME/ruvx/config.toml` atau `~/.config/ruvx/config.toml`. Session memakai `$XDG_DATA_HOME/ruvx/sessions/` atau `~/.local/share/ruvx/sessions/`. Lokasinya sama pada macOS dan Linux. ruvx tidak memuat `.env` atau config eksekusi proyek otomatis.

Contoh profil gateway yang memakai environment variable, tanpa menaruh key di config:

```toml
schema_version = 1

[agent]
max_turns = 30

[providers.gateway]
kind = "compatible"
base_url = "https://gateway.example/v1"

[providers.gateway.auth]
method = "environment"
variable = "GATEWAY_API_KEY"

[providers.gateway.models."your-model-id"]
supports_tools = true
supports_streaming = true
```

Ganti URL dan model ID sesuai layanan. Isi `context_limit` hanya dengan limit yang diketahui benar. Seluruh field, batas tools, dan lokasi data dijelaskan dalam [kontrak konfigurasi](docs/ruvx-v1/07-configuration-and-storage.md).

## Pengujian

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
python3 scripts/verify_terminal.py --binary target/release/ruvx --benchmark
```

Test suite memakai filesystem sementara, mock HTTP/SSE, dan fixture OAuth sintetis. Script PTY menguji onboarding, kerahasiaan input key, paste/Unicode/resize, drag-copy, prioritas Ctrl+C, scroll, approval shell, cancel, resume, serta cleanup raw mode, mouse, dan alternate screen. Opsi benchmark mengukur 20 startup warm-cache, RAM idle selama 10 detik, serta ukuran binary.

Fixture private key di `tests/fixtures/` adalah data publik khusus pengujian signature dan tidak digunakan untuk akun nyata. Pengujian lokal tidak mengonsumsi kuota LLM.

## Dokumentasi

- [Ringkasan fitur](docs/features.md)
- [Rancangan dan keputusan v1](docs/ruvx-v1/README.md)
- [Verifikasi fullscreen dan penggunaan resource](docs/validation-fullscreen.md)
- [Verifikasi core v1 dan batas pengujian](docs/validation-v1.md)
- [Kontrak provider dan autentikasi](docs/ruvx-v1/04-providers-and-auth.md)
- [Roadmap dan gerbang penerimaan](docs/ruvx-v1/09-implementation-roadmap.md)

## Agent Skills dan MCP

- **Agent Skills:** simpan `SKILL.md` dengan YAML frontmatter `name` dan `description` di `.agents/skills/<nama>/` proyek atau `~/.config/ruvx/skills/<nama>/` (mengikuti XDG), atau daftarkan folder lokal melalui `/skills add`. Metadata otomatis tersedia bagi agent; body dan resource dibaca saat diperlukan melalui `read_skill`. Katalog dibatasi 16 KiB; `list_skills` menyediakan paging untuk daftar lengkap. `/skills remove` hanya melepas lokasi tambahan, tanpa menghapus sumber. `/skills enable` dan `/skills disable` mengatur discovery global. Script tidak dijalankan otomatis dan skill tidak dapat memperluas izin.
- **MCP stdio:** gunakan `/mcp add` untuk setup, `/mcp edit <id>` untuk mengubah parameter, serta `/mcp enable|disable|remove <id>` untuk mengelola server. Perubahan tersimpan di config global dan berlaku pada run berikutnya tanpa restart. `/mcp` menampilkan daftar tanpa memulai proses. Dalam Build, agent dapat melakukan discovery dan memanggil tool setelah approval eksplisit untuk setiap operasi. Server berhenti ketika run agent berakhir; outcome terputus memerlukan pemeriksaan dan acknowledgement, bukan replay otomatis.

Lihat [panduan Agent Skills dan MCP](docs/skills-and-mcp.md) untuk contoh lengkap, environment, paging, protokol, dan batas keamanan. MCP HTTP, resources/prompts, installer skill, subagent, undo otomatis, headless, dan Windows native belum didukung.
